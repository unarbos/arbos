// Package embed is arbos's public in-process entrypoint to the Dendrite
// monolith (ADR-0041). It lives inside the Dendrite module so it may import the
// internal/* packages the monolith wiring needs — the exact reason an external
// module cannot embed upstream Dendrite directly. arbos depends on this package
// (via a replace directive during development, or the published fork) and never
// touches Dendrite's internals itself.
//
// Start brings up a loopback-only, SQLite-backed, in-memory-NATS homeserver and
// returns a handle with its base URL; Shutdown tears it down. Federation, TLS,
// and registration policy are the caller's to configure later — this is the
// minimal "a node is its own homeserver" core.
package embed

import (
	"context"
	"fmt"
	"path/filepath"

	"github.com/element-hq/dendrite/appservice"
	"github.com/element-hq/dendrite/federationapi"
	"github.com/element-hq/dendrite/internal/caching"
	"github.com/element-hq/dendrite/internal/httputil"
	"github.com/element-hq/dendrite/internal/sqlutil"
	"github.com/element-hq/dendrite/roomserver"
	"github.com/element-hq/dendrite/setup"
	basepkg "github.com/element-hq/dendrite/setup/base"
	"github.com/element-hq/dendrite/setup/config"
	"github.com/element-hq/dendrite/setup/jetstream"
	"github.com/element-hq/dendrite/setup/process"
	"github.com/element-hq/dendrite/userapi"
	uapi "github.com/element-hq/dendrite/userapi/api"
	"github.com/matrix-org/gomatrixserverlib/spec"
)

// Options configures an embedded homeserver.
type Options struct {
	// ServerName is the Matrix server_name (the part after ':' in user/room
	// IDs). For a node it is the machine's forest URL; for a loopback-only dev
	// server, "localhost" is fine.
	ServerName string
	// DataDir holds the per-component SQLite databases and the media store.
	DataDir string
	// BindAddr is the loopback HTTP listen address, e.g. "127.0.0.1:18008".
	BindAddr string
	// OpenRegistration enables password registration with dummy auth. For a
	// loopback-only homeserver this is how the node registers its own agent and
	// human users before the appservice/identity flow lands.
	OpenRegistration bool
}

// Server is a running embedded homeserver.
type Server struct {
	// BaseURL is the client-server API base, e.g. "http://127.0.0.1:18008".
	BaseURL    string
	processCtx *process.ProcessContext
	userAPI    uapi.UserInternalAPI
	serverName spec.ServerName
}

// CreateAccount provisions a local user directly via the user API — the node
// creating its own agent/human users (like Dendrite's create-account command),
// independent of client-side registration flows. Returns the full Matrix user
// ID (@localpart:server_name). Idempotent callers should tolerate an
// already-exists error.
func (s *Server) CreateAccount(ctx context.Context, localpart, password string) (string, error) {
	var res uapi.PerformAccountCreationResponse
	if err := s.userAPI.PerformAccountCreation(ctx, &uapi.PerformAccountCreationRequest{
		AccountType: uapi.AccountTypeUser,
		Localpart:   localpart,
		ServerName:  s.serverName,
		Password:    password,
	}, &res); err != nil {
		return "", err
	}
	return fmt.Sprintf("@%s:%s", localpart, s.serverName), nil
}

// Start wires and serves the Dendrite monolith in-process. It returns once the
// HTTP listener goroutine is launched; callers should poll BaseURL +
// "/_matrix/client/versions" for readiness.
func Start(opts Options) (*Server, error) {
	if opts.ServerName == "" {
		opts.ServerName = "localhost"
	}
	if opts.BindAddr == "" {
		opts.BindAddr = "127.0.0.1:18008"
	}
	if opts.DataDir == "" {
		return nil, fmt.Errorf("embed: DataDir is required")
	}

	var cfg config.Dendrite
	cfg.Defaults(config.DefaultOpts{Generate: true, SingleDatabase: false})
	cfg.Global.ServerName = spec.ServerName(opts.ServerName)
	cfg.Global.JetStream.InMemory = true
	cfg.SyncAPI.Fulltext.Enabled = false
	if opts.OpenRegistration {
		cfg.ClientAPI.RegistrationDisabled = false
		cfg.ClientAPI.OpenRegistrationWithoutVerificationEnabled = true
	}
	ds := func(name string) config.DataSource {
		return config.DataSource("file:" + filepath.Join(opts.DataDir, name))
	}
	cfg.FederationAPI.Database.ConnectionString = ds("federationapi.db")
	cfg.KeyServer.Database.ConnectionString = ds("keyserver.db")
	cfg.MSCs.Database.ConnectionString = ds("mscs.db")
	cfg.MediaAPI.Database.ConnectionString = ds("mediaapi.db")
	cfg.MediaAPI.BasePath = config.Path(filepath.Join(opts.DataDir, "media"))
	cfg.RoomServer.Database.ConnectionString = ds("roomserver.db")
	cfg.SyncAPI.Database.ConnectionString = ds("syncapi.db")
	cfg.UserAPI.AccountDatabase.ConnectionString = ds("userapi.db")
	cfg.RelayAPI.Database.ConnectionString = ds("relayapi.db")

	cfgErrs := &config.ConfigErrors{}
	cfg.Verify(cfgErrs)
	if len(*cfgErrs) > 0 {
		return nil, fmt.Errorf("embed: config verify: %v", *cfgErrs)
	}

	processCtx := process.NewProcessContext()
	fedClient := basepkg.CreateFederationClient(&cfg, nil)
	httpClient := basepkg.CreateClient(&cfg, nil)
	cm := sqlutil.NewConnectionManager(processCtx, cfg.Global.DatabaseOptions)
	routers := httputil.NewRouters()
	caches := caching.NewRistrettoCache(cfg.Global.Cache.EstimatedMaxSize, cfg.Global.Cache.MaxAge, caching.EnableMetrics)
	natsInstance := jetstream.NATSInstance{}

	rsAPI := roomserver.NewInternalAPI(processCtx, &cfg, cm, &natsInstance, caches, caching.EnableMetrics)
	fsAPI := federationapi.NewInternalAPI(processCtx, &cfg, cm, &natsInstance, fedClient, rsAPI, caches, nil, false)
	keyRing := fsAPI.KeyRing()
	rsAPI.SetFederationAPI(fsAPI, keyRing)
	userAPI := userapi.NewInternalAPI(processCtx, &cfg, cm, &natsInstance, rsAPI, fedClient, caching.EnableMetrics, fsAPI.IsBlacklistedOrBackingOff)
	asAPI := appservice.NewInternalAPI(processCtx, &cfg, &natsInstance, userAPI, rsAPI)
	rsAPI.SetAppserviceAPI(asAPI)
	rsAPI.SetUserAPI(userAPI)

	monolith := setup.Monolith{
		Config: &cfg, Client: httpClient, FedClient: fedClient, KeyRing: keyRing,
		AppserviceAPI: asAPI, FederationAPI: fsAPI, RoomserverAPI: rsAPI, UserAPI: userAPI,
	}
	monolith.AddAllPublicRoutes(processCtx, &cfg, routers, cm, &natsInstance, caches, caching.EnableMetrics)

	httpAddr, err := config.HTTPAddress("http://" + opts.BindAddr)
	if err != nil {
		return nil, fmt.Errorf("embed: parse bind addr: %w", err)
	}
	go basepkg.SetupAndServeHTTP(processCtx, &cfg, routers, httpAddr, nil, nil)

	return &Server{
		BaseURL:    "http://" + opts.BindAddr,
		processCtx: processCtx,
		userAPI:    userAPI,
		serverName: cfg.Global.ServerName,
	}, nil
}

// Shutdown stops the homeserver and waits for its components to finish.
func (s *Server) Shutdown() {
	if s == nil || s.processCtx == nil {
		return
	}
	s.processCtx.ShutdownDendrite()
	s.processCtx.WaitForShutdown()
}
