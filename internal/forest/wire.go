// Package forest is the network of arbos nodes (ADR-0034): the head side — a
// self-hostable registry + relay that gives each joined node an ephemeral
// public URL — and the node side, a client that registers a device identity,
// leases a name, and serves its gateway back through an outbound tunnel.
//
// The wire surface is the subset of docs/account-api-contract.md a forest
// head implements: device registration, signed-challenge token mint, the node
// heartbeat (which carries the lease), and the tunnel handshake. A node built
// against this package works against the hosted head and a self-hosted one
// identically — the head is part of the swappable backend, per ADR-0033.
package forest

// registerReq is POST /v1/devices/register: idempotent on the public key. A
// new key gets a fresh anonymous account (ADR-0033).
type registerReq struct {
	PublicKey string  `json:"public_key"` // base64 (std) ed25519 public key
	Machine   machine `json:"machine"`
}

type machine struct {
	Hostname string `json:"hostname"`
	OS       string `json:"os"`
	Arch     string `json:"arch"`
	Version  string `json:"arbos_version,omitempty"`
}

type registerResp struct {
	AccountID string `json:"account_id"`
	DeviceID  string `json:"device_id"`
}

// challengeResp is GET /v1/devices/challenge: a nonce the device signs to
// prove key possession when minting a token.
type challengeResp struct {
	Nonce     string `json:"nonce"` // base64 (std)
	ExpiresAt string `json:"expires_at"`
}

// tokenReq is POST /v1/devices/token: no bearer; the signature is the auth.
type tokenReq struct {
	DeviceID  string `json:"device_id"`
	Nonce     string `json:"nonce"`     // base64, echoed from the challenge
	Signature string `json:"signature"` // base64 ed25519 sig over the nonce bytes
}

type tokenResp struct {
	AccessToken string `json:"access_token"`
	TokenType   string `json:"token_type"`
	ExpiresIn   int    `json:"expires_in"` // seconds
}

// heartbeatReq is POST /v1/nodes/heartbeat (bearer). For the anonymous tier
// the heartbeat is also the lease: the head assigns a name on first beat and
// extends it on every subsequent one.
type heartbeatReq struct {
	Host string   `json:"host,omitempty"`
	Cwd  string   `json:"cwd,omitempty"`
	Caps []string `json:"caps,omitempty"`
}

// heartbeatResp carries the lease. TTL and cadence are head-controlled so
// abuse pressure is tunable server-side without shipping new node binaries.
type heartbeatResp struct {
	Name             string `json:"name"` // assigned, never chosen (anonymous tier)
	URL              string `json:"url"`  // public base URL for this node
	TTLSeconds       int    `json:"ttl_seconds"`
	HeartbeatSeconds int    `json:"heartbeat_seconds"`
}

// errorResp is the contract's error envelope.
type errorResp struct {
	Error errorBody `json:"error"`
}

type errorBody struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}

// tunnelPath is the relay handshake endpoint: a bearer-authenticated
// WebSocket that becomes a yamux session. The head opens one stream per
// proxied request; the node serves its gateway handler on the accepting side.
const tunnelPath = "/v1/tunnel"
