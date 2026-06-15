// Command head runs the arbos.life account backend: non-KYC, agent-first
// accounts with a prepaid balance, scoped API keys, and an OpenAI-compatible
// inference gateway that meters and forwards to OpenRouter. It is a sibling to
// `cmd/forest` (the relay), not a replacement — run it alongside while the
// account backend is built out.
//
// The OpenRouter key is read from the environment; the intended invocation
// sources it from Doppler:
//
//	doppler run -- head --addr :8090 --db ~/.config/arbos-head/head.db
//
// An agent then points its OpenAI client at this head and uses an arbos key:
//
//	curl -s localhost:8090/v1/signup -d '{"name":"laptop"}'   # -> {api_key, account_id}
//	# base_url = http://localhost:8090/v1 ; api_key = sk-arbos-...
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"syscall"
	"time"

	"github.com/unarbos/arbos/internal/head"
)

func main() {
	addr := flag.String("addr", ":8090", "listen address")
	db := flag.String("db", defaultDBPath(), "sqlite database path")
	orBase := flag.String("openrouter-base", "", "OpenRouter base URL (default https://openrouter.ai/api/v1)")
	marginBps := flag.Int("margin-bps", 0, "markup over upstream cost in basis points (100 = 1%); 0 = pass-through")
	promoUSD := flag.Float64("promo-usd", 0, "signup allowance credited to a new account, in USD")
	mock := flag.Bool("mock", false, "local demo mode: mock inference + simulated funding (no key, no chain, no wallet)")
	flag.Parse()

	orKey := os.Getenv("OPENROUTER_API_KEY")
	if orKey == "" && !*mock {
		fmt.Fprintln(os.Stderr, "head: OPENROUTER_API_KEY is required (try: doppler run -- head ..., or --mock for a local demo)")
		os.Exit(2)
	}
	adminToken := os.Getenv("ARBOS_HEAD_ADMIN_TOKEN") // empty disables the funding endpoint

	store, err := head.Open(*db)
	if err != nil {
		fmt.Fprintf(os.Stderr, "head: %v\n", err)
		os.Exit(1)
	}
	defer func() { _ = store.Close() }()

	cfg := head.Config{
		Store:          store,
		OpenRouterKey:  orKey,
		OpenRouterBase: *orBase,
		MarginBps:      *marginBps,
		PromoMicro:     head.MicroFromUSD(*promoUSD),
		AdminToken:     adminToken,
		// Crypto funding: set ARBOS_EVM_RPC to enable per-account deposit
		// addresses + POST /v1/fund/evm. ARBOS_ETH_USD is dollars per ETH.
		EVMRPCURL:      os.Getenv("ARBOS_EVM_RPC"),
		EVMUSDCAddr:    os.Getenv("ARBOS_USDC_ADDR"),
		EVMChainID:     envInt("ARBOS_EVM_CHAIN_ID", 0),
		EVMETHUSDMicro: head.MicroFromUSD(envFloat("ARBOS_ETH_USD", 3000)),
		EVMMinConf:     envInt("ARBOS_EVM_MIN_CONF", 1),
	}
	if *mock {
		cfg.Upstream = mockUpstream{} // canned replies, no key/network
		cfg.Mock = true               // enables POST /v1/fund/mock (simulated payment)
		fmt.Fprintln(os.Stderr, "head: MOCK MODE — inference and funding are simulated; do not expose publicly")
	}
	h := head.New(cfg)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	srv := &http.Server{Addr: *addr, Handler: h.Handler()}
	errCh := make(chan error, 1)
	go func() { errCh <- srv.ListenAndServe() }()
	fmt.Fprintf(os.Stderr, "arbos head listening on %s (db %s, margin %s bps)\n", *addr, *db, strconv.Itoa(*marginBps))

	select {
	case <-ctx.Done():
		fmt.Fprintln(os.Stderr, "head: shutting down")
		// Drain in-flight requests (an inference stream can be mid-flight)
		// rather than severing them, bounded so a stuck stream can't hang exit.
		shutCtx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutCtx)
	case err := <-errCh:
		if err != nil && !errors.Is(err, http.ErrServerClosed) {
			fmt.Fprintf(os.Stderr, "head: serve: %v\n", err)
			os.Exit(1)
		}
	}
}

// envInt reads an integer env var, falling back to def when unset/invalid.
func envInt(name string, def int64) int64 {
	if v, err := strconv.ParseInt(os.Getenv(name), 10, 64); err == nil {
		return v
	}
	return def
}

// envFloat reads a float env var, falling back to def when unset/invalid.
func envFloat(name string, def float64) float64 {
	if v, err := strconv.ParseFloat(os.Getenv(name), 64); err == nil {
		return v
	}
	return def
}

// defaultDBPath puts the database under the user's config dir, beside the
// forest head's own state.
func defaultDBPath() string {
	dir, err := os.UserConfigDir()
	if err != nil || dir == "" {
		return "head.db"
	}
	return filepath.Join(dir, "arbos-head", "head.db")
}
