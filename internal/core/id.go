package core

// SessionID identifies a conversation session. It is a distinct type (not a
// bare string) so it cannot be silently swapped with other identifiers at call
// sites — data-structures-first type safety.
type SessionID string

// RequestID correlates a mid-turn request emitted by the kernel (e.g.
// ApprovalRequest, ClarifyRequest) with the Intent that answers it
// (ApprovalResponseIntent, ClarifyResponseIntent). Distinct type for the same
// type-safety reason as SessionID. See ADR-0018.
type RequestID string
