package main

import (
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/unarbos/arbos/internal/head"
)

// accountPrefixes are the apex paths the account head owns. They are disjoint
// from everything the forest head serves (its /v1/devices, /v1/nodes, /v1/tunnel,
// /install.sh, /arbos, assets), so the split is unambiguous.
var accountPrefixes = []string{
	"/v1/chat/completions",
	"/v1/models",
	"/v1/signup",
	"/v1/balance",
	"/v1/keys",
	"/v1/fund/",
	"/v1/admin/",
	"/healthz",
}

// mountAccount composes the account head (internal/head) onto the forest
// handler when an OpenRouter key is configured. The key comes from
// OPENROUTER_API_KEY, else <stateDir>/openrouter.key; the admin token (for the
// funding stand-in) from ARBOS_HEAD_ADMIN_TOKEN, else <stateDir>/admin.token;
// the account database lives at <stateDir>/head.db, beside the forest's
// devices.json.
//
// Routing: on the apex host, account-owned paths and a browser hitting "/" go
// to the head (the dashboard); curl/wget at "/", the device APIs, the install
// scripts, and every lease subdomain stay with the forest. Without a key the
// forest handler is returned unchanged, so a key-less self-hoster runs exactly
// as before.
func mountAccount(forestH http.Handler, domain, stateDir string, logf func(string, ...any)) (http.Handler, io.Closer) {
	orKey := strings.TrimSpace(os.Getenv("OPENROUTER_API_KEY"))
	if orKey == "" {
		orKey = readTrimmed(filepath.Join(stateDir, "openrouter.key"))
	}
	if orKey == "" {
		logf("forest: account head disabled (no OpenRouter key) — serving forest only")
		return forestH, nil
	}
	store, err := head.Open(filepath.Join(stateDir, "head.db"))
	if err != nil {
		logf("forest: account head disabled: %v", err)
		return forestH, nil
	}
	adminToken := strings.TrimSpace(os.Getenv("ARBOS_HEAD_ADMIN_TOKEN"))
	if adminToken == "" {
		adminToken = readTrimmed(filepath.Join(stateDir, "admin.token"))
	}
	// Crypto funding (optional): enabled when an EVM RPC + treasury address are
	// present, from env or files beside the state (evm-rpc, treasury-addr,
	// evm-chain-id, eth-usd). Without them the account head simply omits
	// POST /v1/fund/evm.
	evmRPC := firstNonEmpty(os.Getenv("ARBOS_EVM_RPC"), readTrimmed(filepath.Join(stateDir, "evm-rpc")))
	treasury := firstNonEmpty(os.Getenv("ARBOS_TREASURY_ADDR"), readTrimmed(filepath.Join(stateDir, "treasury-addr")))
	ethUSD := parseFloat(firstNonEmpty(os.Getenv("ARBOS_ETH_USD"), readTrimmed(filepath.Join(stateDir, "eth-usd"))), 3000)
	chainID := parseInt(firstNonEmpty(os.Getenv("ARBOS_EVM_CHAIN_ID"), readTrimmed(filepath.Join(stateDir, "evm-chain-id"))), 0)
	if evmRPC != "" && treasury != "" {
		logf("forest: crypto funding live — treasury %s on chain %d (rpc %s)", treasury, chainID, evmRPC)
	}
	acctH := head.New(head.Config{
		Store:          store,
		OpenRouterKey:  orKey,
		AdminToken:     adminToken,
		EVMRPCURL:      evmRPC,
		EVMTreasury:    treasury,
		EVMChainID:     chainID,
		EVMETHUSDMicro: head.MicroFromUSD(ethUSD),
		EVMMinConf:     3,
	}).Handler()

	apex := apexHost(domain)
	logf("forest: account head mounted on %s (db %s/head.db)", apex, stateDir)
	combined := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if apexHost(r.Host) == apex {
			p := r.URL.Path
			if hasAnyPrefix(p, accountPrefixes) || (p == "/" && browserWantsHTML(r)) {
				acctH.ServeHTTP(w, r)
				return
			}
		}
		forestH.ServeHTTP(w, r)
	})
	return combined, store
}

// apexHost lowercases a host[:port] and drops the port, for apex matching.
func apexHost(h string) string {
	h = strings.ToLower(h)
	if host, _, err := net.SplitHostPort(h); err == nil {
		return host
	}
	return h
}

func readTrimmed(path string) string {
	b, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

func parseFloat(s string, def float64) float64 {
	if f, err := strconv.ParseFloat(s, 64); err == nil {
		return f
	}
	return def
}

func parseInt(s string, def int64) int64 {
	if n, err := strconv.ParseInt(s, 10, 64); err == nil {
		return n
	}
	return def
}

func hasAnyPrefix(p string, prefixes []string) bool {
	for _, pre := range prefixes {
		if strings.HasPrefix(p, pre) {
			return true
		}
	}
	return false
}

// browserWantsHTML is the mirror of the forest's install-script sniff: a real
// browser (Accept: text/html, not curl/wget) gets the dashboard at the apex;
// everything else falls through to the forest, so `curl https://arbos.life`
// still pipes the install script unchanged.
func browserWantsHTML(r *http.Request) bool {
	ua := r.Header.Get("User-Agent")
	if strings.Contains(ua, "curl") || strings.Contains(ua, "wget") {
		return false
	}
	return strings.Contains(r.Header.Get("Accept"), "text/html")
}
