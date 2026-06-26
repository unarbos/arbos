// Package matrix is the Matrix substrate adapter (ADR-0041): the ONLY place in
// the tree that knows about Matrix. It translates the kernel's three
// vocabularies (core.Intent in, core.KernelEvent out, core.EventPayload
// persisted) to and from Matrix, and embeds/drives the homeserver. The
// dependency arrow points one way: this package imports internal/core; core
// must never import this package (enforced by TestKernelImportsStaySubstrateFree).
//
// This file is the wire contract — the life.arbos.* event-type schema and the
// three-channel routing — independent of any Matrix client library, so the
// shape is reviewable before the embedding lands. mautrix/dendrite are wired in
// the adapter implementation, not here.
package matrix

import "github.com/unarbos/arbos/internal/core"

// Channel is which Matrix transport a piece of the kernel vocabulary rides
// (ADR-0041 D10). The kernel has three vocabularies with three lifecycles, and
// Matrix has three channels that match them one-for-one.
type Channel uint8

const (
	// ChannelRoom — the durable, shared, attributed room timeline. Content:
	// prose, shared tool results, chat notes, questions/answers, delegation.
	ChannelRoom Channel = iota
	// ChannelToDevice — directed, ephemeral, federating control commands aimed
	// at a single agent (interrupt/steer/compact/set_*/resume).
	ChannelToDevice
	// ChannelLocal — ephemeral presentation rendered only to this node's own
	// frontends (streaming deltas, progress); never federated (D6).
	ChannelLocal
)

// Room-timeline event types (ChannelRoom). Prose (user/assistant) rides the
// standard m.room.message with the structured arbos payload under the
// ContentExt key, so generic clients (Element) render the prose and arbos reads
// the extension — one dual-readable event, not two (D10.2). Everything with no
// prose form is its own life.arbos.* type (generic clients hide it).
const (
	MsgRoomMessage = "m.room.message"
	MsgNotice      = "m.notice" // fallback summary body for raised tool results (D1)

	// ContentExt is the key under an m.room.message's content carrying the
	// arbos structured metadata (turn_id, role, reasoning?, tool_calls?,
	// provider_meta?, citations?). See D10.2.
	ContentExt = "life.arbos"

	EvtToolResult  = "life.arbos.tool_result"  // {call_id, content, is_error}; m.reference -> assistant
	EvtChatNote    = "life.arbos.chat_note"    // human-to-human; excluded from model projection (ADR-0038)
	EvtQuestion    = "life.arbos.question"     // {request_id, questions|approval, reason}
	EvtAnswer      = "life.arbos.answer"       // {request_id, answers}; m.reference -> the question
	EvtTask        = "life.arbos.task"         // delegation Task (thread/child room)
	EvtKernelEvent = "life.arbos.kernel_event" // relayed child events (delegation depth)
	EvtForkedFrom  = "life.arbos.forked_from"  // fork lineage (D11)
	EvtImported    = "life.arbos.imported"     // re-emitted fork prefix (D11)
	EvtSpawnedBy   = "life.arbos.spawned_by"   // owned-spawn provenance tag (D11)
	EvtSurface     = "life.arbos.surface"      // show-tool panel hint (resolved angle #4)
)

// Room-state event types (ChannelRoom, but state not timeline).
const (
	StateBudget = "life.arbos.budget" // cap/policy: slow-changing, owner-set (D13)
)

// EvtSpend is the append-only running-spend tick folded into the fuel gauge
// (D13): a timeline event, never room state, so concurrent decrements are
// conflict-free.
const EvtSpend = "life.arbos.spend"

// To-device event type (ChannelToDevice): one control op, replacing the four
// Set*Intent + interrupt/resume/compact transports (D10, D15c).
const EvtControl = "life.arbos.control"

// RoomType returns the Matrix room-timeline event type for a Room-audience
// kernel event payload kind (D10). Prose maps to m.room.message (with the arbos
// content extension); structured machinery maps to a life.arbos.* type. It is
// only meaningful for kinds whose audience is AudienceRoom — see DefaultAudience
// and the engine's per-event raise logic. ok is false for kinds that never
// federate (their durable home is the Local overlay, not the room).
func RoomType(k core.EventKind) (eventType string, ok bool) {
	switch k {
	case core.EventUserMessage, core.EventAssistantMessage:
		return MsgRoomMessage, true
	case core.EventToolResult:
		// Local by default (DefaultAudience); when raised to a shared working
		// room it lands as this type (with an m.notice fallback body).
		return EvtToolResult, true
	case core.EventChatNote:
		return EvtChatNote, true
	case core.EventUsage, core.EventCompressed, core.EventContext,
		core.EventInterrupted, core.EventConfig, core.EventBranchAnchor:
		// Private trajectory / bookkeeping: never a room event (D1). These live
		// in the agent's Local overlay; the room is the shared window only.
		return "", false
	}
	return "", false
}
