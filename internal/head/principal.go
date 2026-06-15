package head

import (
	"context"
	"net/http"
	"strings"
)

// Principal is who a request acts as, resolved from a credential. Today the
// only credential is an API key; a device token (forest) or a browser session
// can resolve to the same shape later without changing callers.
type Principal struct {
	AccountID string
	KeyID     string // the api-key id used, for usage attribution
}

// resolve pulls the bearer token off the request and resolves it to a
// Principal. A missing or malformed Authorization header, or an unknown/revoked
// key, returns (nil, false) — the caller answers 401 without saying which.
func (h *Head) resolve(ctx context.Context, r *http.Request) (*Principal, bool) {
	tok, ok := bearer(r)
	if !ok {
		return nil, false
	}
	key, err := h.store.ResolveKey(ctx, tok)
	if err != nil {
		return nil, false
	}
	return &Principal{AccountID: key.AccountID, KeyID: key.ID}, true
}

// requirePrincipal resolves the caller or answers 401 with the standard
// envelope. The one place every authenticated handler starts: it returns the
// principal and true, or writes the error and returns false so the handler can
// just `return`.
func (h *Head) requirePrincipal(w http.ResponseWriter, r *http.Request) (*Principal, bool) {
	princ, ok := h.resolve(r.Context(), r)
	if !ok {
		writeAPIErr(w, http.StatusUnauthorized, "unauthorized", "valid arbos API key required")
	}
	return princ, ok
}

// bearer extracts a "Bearer <token>" credential, also accepting a bare key in
// the header (some agent SDKs send the key without the scheme).
func bearer(r *http.Request) (string, bool) {
	h := strings.TrimSpace(r.Header.Get("Authorization"))
	if h == "" {
		return "", false
	}
	// The "Bearer" scheme is case-insensitive (RFC 7235); accept any casing.
	if len(h) >= 7 && strings.EqualFold(h[:7], "Bearer ") {
		return strings.TrimSpace(h[7:]), true
	}
	if strings.HasPrefix(h, keyPrefix) {
		return h, true
	}
	return "", false
}
