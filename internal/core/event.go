// Package core holds the kernel's foundational data types: the event log,
// the conversation projection, and the intent/event vocabulary that flows in
// and out of the engine. It depends on nothing else in the tree — every other
// package depends on it.
package core

import (
	"errors"
	"time"
)

// CurrentEventVersion is the schema version stamped on every event the current
// build writes. The persisted log is append-only, so a reader must know which
// writer produced a row to upcast it correctly when the Event/payload shapes
// evolve (new kinds, multimodal content, etc.). Bump this and add an upcasting
// rule whenever a payload's persisted shape changes. See ADR-0010.
const CurrentEventVersion = 1

// EventKind is the persisted discriminator for an event's payload. It is
// derived from the payload type (Payload.Kind()) and serialized as a column so
// stores can index/filter (e.g. FTS only over message kinds) without decoding
// the payload.
type EventKind string

const (
	EventUserMessage      EventKind = "user_message"
	EventAssistantMessage EventKind = "assistant_message"
	EventToolResult       EventKind = "tool_result"
	EventUsage            EventKind = "usage"
	EventCompressed       EventKind = "compressed"
	EventContext          EventKind = "context"
	EventInterrupted      EventKind = "interrupted"
	EventConfig           EventKind = "config"
	EventChatNote         EventKind = "chat_note"
)

// EventPayload is a sealed sum type: the kernel's event payloads are a closed
// set, each carrying exactly the data its kind needs. This makes "exactly one
// payload, matching the kind" a structural guarantee enforced by the compiler
// (you cannot build an event with two payloads or none), consistent with how
// Intent and KernelEvent are modeled. Kind() doubles as the seal marker.
type EventPayload interface {
	Kind() EventKind
}

// MessagePayload carries a user or assistant message. The concrete kind is
// derived from the role so user and assistant turns share one payload type.
type MessagePayload struct{ Message Message }

func (p MessagePayload) Kind() EventKind {
	if p.Message.Role == RoleUser {
		return EventUserMessage
	}
	return EventAssistantMessage
}

// ToolResultPayload carries the outcome of a dispatched tool call.
type ToolResultPayload struct{ Result ToolResult }

func (ToolResultPayload) Kind() EventKind { return EventToolResult }

// UsagePayload records token accounting for a completed LLM response. Usage
// lives in the log (not only on session metadata) so trajectories and audits
// reconstruct it from the event stream. See ADR-0011.
type UsagePayload struct{ Usage Usage }

func (UsagePayload) Kind() EventKind { return EventUsage }

// CompressionPayload records that a span of earlier events was folded into a
// summary. ReplacedSeqLo/Hi bound the compressed range (inclusive).
type CompressionPayload struct {
	Summary       string
	ReplacedSeqLo int64
	ReplacedSeqHi int64
}

func (CompressionPayload) Kind() EventKind { return EventCompressed }

// Segment is a provenance-tagged piece of injected context (memory recall, the
// background-job table, a skill body, a retrieved document). Source identifies
// the origin so the projection can fence it and so the most recent segment per
// source supersedes older ones. The kernel never interprets Content.
type Segment struct {
	Source  string
	Content string
}

// SourceJobs is the Segment.Source for the workspace's background-job table
// (ADR-0032). Like SourceMemory, the stable source name is load-bearing: the
// latest-per-source projection replaces, rather than accumulates, the table
// each time it is injected, so the prompt always shows one current job table.
const SourceJobs = "jobs"

// SourcePlan is the Segment.Source for the agent's plan forest (internal/plan).
// The same latest-per-source rule is what makes intent survive compaction: the
// projection is re-injected each turn from the store, never accumulated in the
// conversation.
const SourcePlan = "plan"

// ContextPayload records context that was injected into a turn (memory, skills,
// retrieval). It is logged — rather than assembled ephemerally — so a replayed
// session reproduces exactly what the model saw. Projection renders only the
// latest segment per source (older injections stay in the log for audit but are
// superseded), keeping the live prompt lean and prefix-cache stable. See
// ADR-0015.
type ContextPayload struct{ Segments []Segment }

func (ContextPayload) Kind() EventKind { return EventContext }

// InterruptPayload marks that a turn was cancelled before completing.
type InterruptPayload struct{ Reason string }

func (InterruptPayload) Kind() EventKind { return EventInterrupted }

// ConfigPayload records the turn configuration the model actually ran with:
// the resolved model, the system prompt, and the offered toolset. These live
// in engine config (code that evolves with the repo), so without this event a
// historical log could only be reconstructed against *today's* prompt and
// tools — silently wrong as training data. The engine appends one whenever the
// effective config differs from the last logged one (in practice once per
// session, plus on model switches and toolset changes), which keeps every
// exported trajectory self-contained. It has no conversational projection.
type ConfigPayload struct {
	Model        string
	SystemPrompt string
	Tools        []ToolSchema
	// WebSearch records whether provider-side web search was offered this turn.
	// Part of "what the model saw", so a toggle change logs a fresh config event
	// and an exported trajectory is self-contained. See ADR-0027.
	WebSearch bool
	// WebFetch records whether provider-side web fetch was offered this turn,
	// for the same trajectory-fidelity reason as WebSearch.
	WebFetch bool
	// ImageGen records whether provider-side image generation was offered this
	// turn, for the same trajectory-fidelity reason as WebSearch.
	ImageGen bool
}

func (ConfigPayload) Kind() EventKind { return EventConfig }

// ChatNotePayload is a human-to-human side-chat line — collaborators on the
// same session talking to EACH OTHER, not prompting the agent. It is logged
// (so it replays and interleaves by Seq like any event) but is deliberately
// EXCLUDED from ProjectEvent, so it never enters the model context or
// compaction. It reuses Message for the Author/attachment apparatus; Role is
// RoleUser for shape only, and Kind() is its own EventChatNote regardless of
// role, so Validate's user/assistant message check does not apply to it.
type ChatNotePayload struct{ Message Message }

func (ChatNotePayload) Kind() EventKind { return EventChatNote }

// LatestConfig returns the most recent ConfigPayload in the log, reporting
// whether one exists. The latest entry is authoritative: a config event
// supersedes earlier ones the same way the projection's latest-per-source rule
// works for injected context.
func LatestConfig(events []Event) (ConfigPayload, bool) {
	for i := len(events) - 1; i >= 0; i-- {
		if p, ok := events[i].Payload.(ConfigPayload); ok {
			return p, true
		}
	}
	return ConfigPayload{}, false
}

// Event is the spine of the kernel: sessions are an append-only sequence of
// these. The conversation a provider sees is a projection of the log (see
// Project), never a separately mutated structure.
type Event struct {
	ID        int64 // store-assigned
	SessionID SessionID
	Seq       int64 // monotonic within a session, store-assigned
	// TurnID groups every event produced by one user prompt (user message ->
	// context -> assistant -> tool results -> assistant ...). Assigned by the
	// engine (not the store) because a turn spans multiple appends. It is the
	// join key that isolates "everything that happened in this turn" for a
	// debugging agent and the unit of deterministic replay. Zero means the event
	// was not produced inside an engine turn (e.g. an out-of-band injection);
	// it is intentionally not enforced by Validate so non-engine writers and
	// fixtures stay valid. Operational ids (trace_id) ride in context and are
	// stamped on logs/spans, never on this domain type. See docs/12-observability.md.
	TurnID    int64
	Version   int // schema version of this row; see CurrentEventVersion
	CreatedAt time.Time
	Payload   EventPayload
}

// Validate enforces the persistable invariants. Stores MUST call it on append
// so a malformed event never reaches the log (the source of truth).
func (e *Event) Validate() error {
	if e.Payload == nil {
		return errors.New("event has nil payload")
	}
	if e.SessionID == "" {
		return errors.New("event has empty session id")
	}
	if e.Version == 0 {
		return errors.New("event missing schema version")
	}
	// A persisted message must be a user or assistant turn. System and tool-role
	// messages are produced by projection, never stored; a stored MessagePayload
	// with any other role would corrupt Kind() (which maps any non-user role to
	// "assistant") and the projection. The "malformed event never reaches the
	// log" guarantee must not depend on a store's incidental session check.
	if mp, ok := e.Payload.(MessagePayload); ok {
		if mp.Message.Role != RoleUser && mp.Message.Role != RoleAssistant {
			return errors.New("message payload role must be user or assistant")
		}
	}
	return nil
}

// Per-kind constructors are the only sanctioned way the engine builds events.
// They stamp the current schema version so no caller can forget it.

func newEvent(sid SessionID, now time.Time, p EventPayload) *Event {
	return &Event{SessionID: sid, Version: CurrentEventVersion, CreatedAt: now, Payload: p}
}

func NewMessageEvent(sid SessionID, m Message, now time.Time) *Event {
	return newEvent(sid, now, MessagePayload{Message: m})
}

func NewToolResultEvent(sid SessionID, r ToolResult, now time.Time) *Event {
	return newEvent(sid, now, ToolResultPayload{Result: r})
}

func NewUsageEvent(sid SessionID, u Usage, now time.Time) *Event {
	return newEvent(sid, now, UsagePayload{Usage: u})
}

func NewCompressionEvent(sid SessionID, summary string, lo, hi int64, now time.Time) *Event {
	return newEvent(sid, now, CompressionPayload{Summary: summary, ReplacedSeqLo: lo, ReplacedSeqHi: hi})
}

func NewContextEvent(sid SessionID, segments []Segment, now time.Time) *Event {
	return newEvent(sid, now, ContextPayload{Segments: segments})
}

func NewInterruptEvent(sid SessionID, reason string, now time.Time) *Event {
	return newEvent(sid, now, InterruptPayload{Reason: reason})
}

func NewConfigEvent(sid SessionID, p ConfigPayload, now time.Time) *Event {
	return newEvent(sid, now, p)
}

func NewChatNoteEvent(sid SessionID, m Message, now time.Time) *Event {
	return newEvent(sid, now, ChatNotePayload{Message: m})
}
