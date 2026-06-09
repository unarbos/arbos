package core

import "encoding/json"

const redacted = "[REDACTED]"

// SecretRef is an opaque handle to a secret. It is safe to log, persist in
// config, embed in a Grant, or pass in tool arguments: it carries NO secret
// material, only a name a SecretProvider knows how to resolve. The agent (and
// its LLM context) only ever sees refs — never values.
type SecretRef struct {
	Name string
}

func (r SecretRef) String() string { return "secret://" + r.Name }

// SecretValue wraps secret material so it cannot leak by accident. Every
// stringer/marshaler redacts; Expose is the single audited accessor; Destroy
// zeroes the backing bytes. Resolved values are short-lived — fetched at the
// boundary, used, and destroyed — never persisted by the kernel.
type SecretValue struct {
	b []byte
}

func NewSecretValue(b []byte) SecretValue { return SecretValue{b: b} }

func (SecretValue) String() string   { return redacted }
func (SecretValue) GoString() string { return redacted }

// MarshalText / MarshalJSON guarantee a SecretValue can never be serialized
// into config, the event log, or an API payload by accident.
func (SecretValue) MarshalText() ([]byte, error) { return []byte(redacted), nil }
func (SecretValue) MarshalJSON() ([]byte, error) { return json.Marshal(redacted) }

// Expose returns the raw bytes. This is the ONLY accessor and the auditable
// choke point — callers should be few and greppable (the broker's injector).
func (s SecretValue) Expose() []byte { return s.b }

// Destroy zeroes the backing bytes. Call after applying the secret at the
// boundary. Copies share the backing array, so Destroy clears them all.
func (s SecretValue) Destroy() {
	for i := range s.b {
		s.b[i] = 0
	}
}
