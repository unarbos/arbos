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
	// EVM funding (optional). When EVMRPCURL and EVMTreasury are both set,
	// POST /v1/fund/evm credits an account for confirmed native-coin deposits to
	// the treasury, verified against the JSON-RPC endpoint. EVMETHUSDMicro is the
	// micro-USD value of 1 ETH used to convert a deposit to credits.
	EVMRPCURL      string
	EVMTreasury    string
	EVMChainID     int64
	EVMETHUSDMicro int64
	EVMMinConf     int64
}

// Head is the account backend's HTTP surface: the inference gateway, agent
// signup, key + balance reads, and the admin funding stand-in. It owns no
// device leases or tunnels — that is the forest relay's job, left untouched.
type Head struct {
	store       *Store
	upstream    Upstream
	evm         *evmFunder // nil unless EVM funding is configured
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
	if cfg.EVMRPCURL != "" && cfg.EVMTreasury != "" {
		evm = newEVMFunder(cfg.EVMRPCURL, cfg.EVMTreasury, cfg.EVMChainID, cfg.EVMETHUSDMicro, cfg.EVMMinConf)
	}
	return &Head{
		store:       cfg.Store,
		upstream:    up,
		evm:         evm,
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
		mux.HandleFunc("GET /v1/fund/info", h.handleFundInfo)
		mux.HandleFunc("POST /v1/fund/evm", h.handleFundEVM)
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
