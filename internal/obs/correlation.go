// Package obs holds observability primitives: turn correlation IDs and the
// redacting slog handler used by every pi host entrypoint.
package obs

import (
	"context"
	"crypto/rand"
	"encoding/hex"
)

// Correlation identifies one turn across all observability planes.
type Correlation struct {
	SessionID string
	TurnID    int64
	// TraceID is an operational id, unique per turn. It is stamped on logs and
	// (later) spans, but intentionally NOT persisted on the domain core.Event —
	// the domain joins on SessionID+TurnID.
	TraceID string
}

type ctxKey struct{}

// With returns a context carrying the turn's correlation. The engine attaches
// this once per prompt; everything downstream reads it via From.
func With(ctx context.Context, c Correlation) context.Context {
	return context.WithValue(ctx, ctxKey{}, c)
}

// From extracts the turn correlation, if present.
func From(ctx context.Context) (Correlation, bool) {
	c, ok := ctx.Value(ctxKey{}).(Correlation)
	return c, ok
}

// NewTraceID returns a random hex id for one turn's operational trace.
func NewTraceID() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}
