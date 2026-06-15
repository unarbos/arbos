package head

import (
	"encoding/json"
	"net/http"
)

// Config configures a Head. The OpenRouter key is the only hard requirement;
// everything else has a sensible default.
type Config struct {
	// Store is the open database (head.Open).
	Store *Store
	// OpenRouterKey is the head operator's upstream key, attached server-side
	// to every forwarded call. Sourced from the environment (e.g. doppler run
	// -- head ...), never from the agent.
	OpenRouterKey string
	// OpenRouterBase overrides the upstream base URL (default the public one).
	OpenRouterBase string
	// MarginBps marks up the upstream cost in basis points (100 = 1%). Zero is
	// a pure pass-through — "simple forward through OpenRouter".
	MarginBps int
	// PromoMicro is the signup allowance credited to a new account so a fresh
	// key can make a call before funding lands. Zero disables it.
	PromoMicro int64
	// AdminToken guards the out-of-band funding endpoint. Empty disables it.
	AdminToken string
	// Upstream overrides the inference forwarder. Nil builds the OpenRouter
	// forwarder from OpenRouterKey/OpenRouterBase; a test injects a fake here to
	// exercise the metering path without a live endpoint.
	Upstream Upstream
	// Mock enables a local demo mode: a simulated funding endpoint
	// (POST /v1/fund/mock) that credits without a chain or wallet. Never set
	// this in production — it is a free faucet by design.
	Mock bool
	// EVM funding (optional). When EVMRPCURL and EVMTreasury are both set,
	// POST /v1/fund/evm credits an account for confirmed native-coin deposits to
	// the treasury, verified against the JSON-RPC endpoint. EVMETHUSDMicro is the
	// micro-USD value of 1 ETH used to convert a deposit to credits.
	EVMRPCURL      string
	EVMUSDCAddr    string // USDC token contract ("" disables USDC deposits)
	EVMChainID     int64
	EVMETHUSDMicro int64
	EVMMinConf     int64
}

// APIPrefixes are the apex path prefixes the account head owns. It lives beside
// Handler (the routes below) so a composing host — cmd/forest, which mounts the
// head onto the forest relay's listener — routes by the same list the routes
// are registered under, and the two can't drift. Disjoint from everything the
// forest head serves (/v1/devices, /v1/nodes, /v1/tunnel, install, assets).
var APIPrefixes = []string{
	"/v1/chat/completions",
	"/v1/models",
	"/v1/signup",
	"/v1/balance",
	"/v1/keys",
	"/v1/fund/",
	"/v1/admin/",
	"/healthz",
}

// Head is the account backend's HTTP surface: the inference gateway, agent
// signup, key + balance reads, and the admin funding stand-in. It owns no
// device leases or tunnels — that is the forest relay's job, left untouched.
type Head struct {
	store       *Store
	upstream    Upstream
	evm         *evmFunder // nil unless EVM funding is configured
	mock        bool       // local demo mode: simulated funding
	marginBps   int
	promoMicro  int64
	adminToken  string
	modelsCache *modelsCache
}

// New builds a Head from config. The upstream defaults to the OpenRouter
// forwarder unless cfg.Upstream injects one.
func New(cfg Config) *Head {
	up := cfg.Upstream
	if up == nil {
		up = newOpenRouter(cfg.OpenRouterBase, cfg.OpenRouterKey)
	}
	var evm *evmFunder
	if cfg.EVMRPCURL != "" {
		evm = newEVMFunder(cfg.EVMRPCURL, cfg.EVMUSDCAddr, cfg.EVMChainID, cfg.EVMETHUSDMicro, cfg.EVMMinConf)
	}
	return &Head{
		store:       cfg.Store,
		upstream:    up,
		evm:         evm,
		mock:        cfg.Mock,
		marginBps:   cfg.MarginBps,
		promoMicro:  cfg.PromoMicro,
		adminToken:  cfg.AdminToken,
		modelsCache: &modelsCache{},
	}
}

// Handler returns the head's HTTP routes. The inference gateway is
// OpenAI-compatible under /v1 so an agent points its OpenAI base_url here and
// uses an arbos key, unchanged.
func (h *Head) Handler() http.Handler {
	mux := http.NewServeMux()

	// Inference gateway (OpenAI-compatible).
	mux.HandleFunc("POST /v1/chat/completions", h.handleChatCompletions)
	mux.HandleFunc("GET /v1/models", h.handleModels)

	// Account surface.
	mux.HandleFunc("POST /v1/signup", h.handleSignup)
	mux.HandleFunc("GET /v1/balance", h.handleBalance)
	mux.HandleFunc("GET /v1/keys", h.handleListKeys)
	mux.HandleFunc("POST /v1/keys", h.handleMintKey)
	mux.HandleFunc("POST /v1/keys/{id}/revoke", h.handleRevokeKey)

	// Crypto funding: credit an account for a confirmed on-chain deposit.
	// Registered only when an EVM endpoint + treasury are configured.
	if h.evm != nil {
		mux.HandleFunc("POST /v1/fund/evm", h.handleFundEVM)
	}
	// Funding info is served whenever any funding rail (crypto or mock) is on.
	if h.evm != nil || h.mock {
		mux.HandleFunc("GET /v1/fund/info", h.handleFundInfo)
	}
	// Local demo: simulated funding, no chain/wallet. Gated on mock mode.
	if h.mock {
		mux.HandleFunc("POST /v1/fund/mock", h.handleFundMock)
	}
	// Out-of-band funding stand-in (admin-gated): manual credits and the path
	// other rails (TAO, card) will mirror.
	mux.HandleFunc("POST /v1/admin/credit", h.handleAdminCredit)

	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("ok"))
	})
	// The account dashboard at the apex — the human-facing home that the API
	// surface above powers.
	mux.HandleFunc("GET /", h.handleDashboard)
	return mux
}

// handleDashboard serves the single-page account dashboard. Only the exact
// apex path renders it; anything else under it is a 404 (the API owns its own
// explicit routes).
func (h *Head) handleDashboard(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	b, err := siteFS.ReadFile("site/index.html")
	if err != nil {
		http.Error(w, "dashboard unavailable", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	// The dashboard holds the API key in JS-reachable storage, so lock the
	// origin down: same-origin only, inline script/style (the page is fully
	// self-contained), no framing — defense-in-depth against any future XSS.
	w.Header().Set("Content-Security-Policy",
		"default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'self'; frame-ancestors 'none'")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Referrer-Policy", "no-referrer")
	_, _ = w.Write(b)
}

// writeJSON writes v as a JSON response with the given status.
func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// writeAPIErr writes the contract's error envelope:
// {"error":{"type":"...","message":"..."}}.
func writeAPIErr(w http.ResponseWriter, status int, typ, msg string) {
	writeJSON(w, status, map[string]any{
		"error": map[string]string{"type": typ, "message": msg},
	})
}
