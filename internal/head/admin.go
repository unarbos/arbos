package head

import (
	"crypto/subtle"
	"encoding/json"
	"net/http"
	"strings"
)

// handleSignup is the agent-first front door: one unauthenticated call creates
// an account and its first API key, returning the secret exactly once. No
// email, no human, no KYC — an agent can go from nothing to a usable key in a
// single request, then be funded by crypto. The signup promo (if configured)
// is credited so a fresh key can make at least one call before funding lands.
func (h *Head) handleSignup(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Name string `json:"name"`
	}
	_ = json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<16)).Decode(&req)

	acct, err := h.store.CreateAccount(r.Context())
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not create account")
		return
	}
	if h.promoMicro > 0 {
		// Idempotent per account; harmless if it races a retry.
		_ = h.store.Credit(r.Context(), acct.ID, h.promoMicro, reasonPromo, "promo:"+acct.ID, nil)
	}
	name := strings.TrimSpace(req.Name)
	if name == "" {
		name = "default"
	}
	key, secret, err := h.store.MintKey(r.Context(), acct.ID, name)
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not mint key")
		return
	}
	bal, _ := h.store.Balance(r.Context(), acct.ID)
	writeJSON(w, http.StatusCreated, map[string]any{
		"account_id":    acct.ID,
		"api_key":       secret, // shown once; only the hash is stored
		"key_id":        key.ID,
		"balance_micro": bal,
	})
}

// handleBalance returns the caller's account balance and recent spend, keyed by
// the presented API key.
func (h *Head) handleBalance(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	bal, err := h.store.Balance(r.Context(), princ.AccountID)
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "balance lookup failed")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"account_id":    princ.AccountID,
		"balance_micro": bal,
		"balance_usd":   float64(bal) / microPerUSD,
	})
}

// handleListKeys lists the caller's keys (never the secrets).
func (h *Head) handleListKeys(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	keys, err := h.store.ListKeys(r.Context(), princ.AccountID)
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not list keys")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"keys": keys})
}

// handleMintKey issues another key on the caller's account, returning the
// secret once. The minting key authenticates; the new key inherits the account.
func (h *Head) handleMintKey(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	var req struct {
		Name string `json:"name"`
	}
	_ = json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<16)).Decode(&req)
	name := strings.TrimSpace(req.Name)
	if name == "" {
		name = "key"
	}
	key, secret, err := h.store.MintKey(r.Context(), princ.AccountID, name)
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not mint key")
		return
	}
	writeJSON(w, http.StatusCreated, map[string]any{"api_key": secret, "key_id": key.ID})
}

// handleRevokeKey marks one of the caller's keys dead. Idempotent and scoped to
// the caller's account, so a key id from another account is a silent no-op.
func (h *Head) handleRevokeKey(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	if err := h.store.RevokeKey(r.Context(), princ.AccountID, r.PathValue("id")); err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not revoke key")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleAdminCredit funds an account out of band — the stand-in for the crypto
// watcher until it lands (P2). Guarded by a shared admin token so it is not an
// open faucet; the ref makes a credit idempotent across retries.
func (h *Head) handleAdminCredit(w http.ResponseWriter, r *http.Request) {
	if h.adminToken == "" || !adminAuthed(r, h.adminToken) {
		writeAPIErr(w, http.StatusUnauthorized, "unauthorized", "admin token required")
		return
	}
	var req struct {
		AccountID string  `json:"account_id"`
		USD       float64 `json:"usd"`
		Ref       string  `json:"ref"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<16)).Decode(&req); err != nil {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "malformed body")
		return
	}
	if req.AccountID == "" || req.USD <= 0 {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "account_id and a positive usd are required")
		return
	}
	ref := req.Ref
	if ref == "" {
		ref = "admin:" + newID("credit")
	}
	amountMicro := MicroFromUSD(req.USD)
	if err := h.store.Credit(r.Context(), req.AccountID, amountMicro, reasonFunding, ref, map[string]any{"source": "admin"}); err != nil {
		// A failure here is almost always an unknown account_id (FK violation);
		// say that without leaking the driver error.
		writeAPIErr(w, http.StatusBadRequest, "credit_failed", "could not credit account (check account_id)")
		return
	}
	bal, _ := h.store.Balance(r.Context(), req.AccountID)
	writeJSON(w, http.StatusOK, map[string]any{"account_id": req.AccountID, "balance_micro": bal})
}

// adminAuthed checks the X-Admin-Token header against the configured secret in
// constant time.
func adminAuthed(r *http.Request, token string) bool {
	got := r.Header.Get("X-Admin-Token")
	if got == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(got), []byte(token)) == 1
}
