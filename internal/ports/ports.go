// Package ports declares the kernel's outbound interfaces (hexagonal "ports").
// The engine depends only on these; concrete adapters are wired at the edge
// (internal/provider/*, internal/sqlite, internal/tool/coding, internal/agent/pi).
// internal/fake supplies deterministic test doubles for every port.
package ports

import (
	"context"
	"errors"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// ErrSessionNotFound is the sentinel a SessionStore.Get returns when no session
// exists for the id. Callers MUST distinguish it (errors.Is) from transient
// store failures: treating every error as "not found" silently forks or
// clobbers sessions when a real store (e.g. SQLite "database is locked")
// hiccups.
var ErrSessionNotFound = errors.New("session not found")

// LLMProvider turns a provider-neutral request into a stream of chunks. The
// returned channel is closed when the response completes; implementations must
// honor ctx cancellation (an interrupt cancels the turn's context).
type LLMProvider interface {
	Name() string
	Capabilities() Capabilities
	Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error)
}

// Capabilities advertises what a provider supports so the engine can adapt the
// request (vision payloads, reasoning config, tool calling) without per-provider
// branching in the loop.
type Capabilities struct {
	Vision    bool
	Reasoning bool
	Tools     bool
}

// ToolRuntime exposes the available tools and dispatches calls. Dispatch must
// never panic to the caller and must respect ctx cancellation.
type ToolRuntime interface {
	Schemas() []core.ToolSchema
	Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult
}

// ConflictAnalyzer is an OPTIONAL ToolRuntime capability for finer-grained
// parallel scheduling than the coarse ToolSchema.ReadOnly flag. Access returns
// the resources a specific call reads and writes — derived from its args, which
// only the tool layer understands, keeping the kernel arg-agnostic. The engine
// runs two calls concurrently only when their access sets do not conflict
// (core.AccessSet.Conflicts).
//
// A runtime that does not implement it falls back to ReadOnly-based batching
// (read-only calls parallel, anything else serialized), so this is purely
// additive: it widens safe parallelism (e.g. edits to distinct files) without
// changing the contract for runtimes that cannot describe a call's footprint.
type ConflictAnalyzer interface {
	Access(call core.ToolCall) core.AccessSet
}

// AccessOf is the single home for a call's scheduling footprint: a runtime that
// implements ConflictAnalyzer answers for itself; otherwise the footprint is
// derived from the tool's advertised ReadOnly (read-only -> empty set,
// conflicts with nothing; mutating or unknown -> unbounded, conflicts with
// everything). Every layer that schedules — the engine and the composing
// runtimes (Multi, Filter) — derives footprints through this one function, so
// the fallback semantics cannot drift between layers.
func AccessOf(rt ToolRuntime, call core.ToolCall) core.AccessSet {
	if an, ok := rt.(ConflictAnalyzer); ok {
		return an.Access(call)
	}
	for _, s := range rt.Schemas() {
		if s.Name == call.Name {
			if s.ReadOnly {
				return core.AccessSet{}
			}
			break
		}
	}
	return core.AccessSet{Unknown: true}
}

// SessionStore persists sessions and their event logs. AppendEvent assigns Seq
// and ID and MUST reject events that fail core.Event.Validate. Implementations
// must be safe for concurrent use across sessions; the engine guarantees
// single-writer access per session via the session actor.
type SessionStore interface {
	CreateSession(ctx context.Context, s core.Session) error
	Get(ctx context.Context, sessionID core.SessionID) (core.Session, error)
	// UpdateSession persists mutable session metadata (the derived token-count
	// mirror, UpdatedAt, and the active -> ended lifecycle transition). Note
	// compression does NOT use this: it is an in-place log append, not a status
	// change (see ADR-0014).
	UpdateSession(ctx context.Context, s core.Session) error
	AppendEvent(ctx context.Context, e *core.Event) error
	Events(ctx context.Context, sessionID core.SessionID) ([]core.Event, error)
}

// ContextPolicy owns the single decision of WHEN and HOW MUCH to compress — the
// seam that in Hermes was scattered across four call sites. A nil policy means
// "never compress". The actual summarization runs via an auxiliary model in the
// compression phase; this port only owns the trigger and the target span, which
// the engine turns into a CompressionPayload. Declared now to lock the seam;
// engine wiring lands with the auxiliary-model phase.
type ContextPolicy interface {
	// ShouldCompress reports whether the projected conversation is over budget.
	ShouldCompress(totalTokens int, msgs []core.Message) bool
	// CompressibleRange returns the inclusive [lo,hi] event-seq span to fold
	// (typically the oldest turns), leaving recent turns intact. ok=false means
	// there is nothing safe to compress yet.
	CompressibleRange(events []core.Event) (lo, hi int64, ok bool)
}

// Summarizer condenses a span of conversation into a short summary the engine
// stores as a CompressionPayload (ADR-0014). It is the "auxiliary model" of the
// compression phase, behind an interface so it can be a cheap local model, the
// main provider, or a deterministic stub in tests. Optional: with a
// ContextPolicy but no Summarizer the engine falls back to a trivial marker
// summary (compression still happens; it just isn't model-written).
type Summarizer interface {
	Summarize(ctx context.Context, msgs []core.Message) (string, error)
}

// ApprovalPolicy decides whether a tool call may run unattended or must pause
// for human confirmation (suspend-and-await, ADR-0018). A nil policy means
// nothing requires approval. Reason is surfaced to the user in the
// ApprovalRequest. Kept a pure decision so it is trivially testable and so the
// engine owns the await control-flow, not the policy.
type ApprovalPolicy interface {
	Requires(call core.ToolCall) (reason string, required bool)
}

// Observer receives every KernelEvent the engine emits, for structured logging,
// metrics, and traces — the operational plane that sits beside the domain event
// log (ADR-0017). A nil Observer disables observation. Correlation (session,
// turn, trace ids) rides in ctx (obs.Correlation), so the Observer never needs
// it on the event. Implementations MUST be cheap and non-blocking and safe for
// concurrent use: one Observer is shared across all session actors.
type Observer interface {
	ObserveEvent(ctx context.Context, ev core.KernelEvent)
}

// Clock is injected so turns are deterministically testable (replay uses a
// fixed clock; production uses the wall clock). Implementations MUST be safe for
// concurrent use: one Clock is shared across all session actors.
type Clock interface {
	Now() time.Time
}

// SecretProvider resolves a SecretRef to its value from a trusted backing store
// (OS keychain, 1Password, Doppler, an age-encrypted file, or env in dev). The
// kernel never persists the value and Destroy()s it after use at the boundary.
// Implementations must never return the value through any path other than the
// returned core.SecretValue (no logging, no error message echo). See the secret
// package's Broker, which is the only component that binds a value to an
// outbound request. ADR-0016.
type SecretProvider interface {
	Name() string
	Resolve(ctx context.Context, ref core.SecretRef) (core.SecretValue, error)
}
