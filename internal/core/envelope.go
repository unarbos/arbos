package core

// Envelope wraps a KernelEvent with delivery metadata as it leaves the kernel.
// SessionID names the session that produced the event. Depth is 0 for the
// session a frontend is directly attached to and increments by one each time an
// event is relayed up from a delegated child agent (ADR-0013), so a frontend
// can render nested sub-agent activity (indentation, collapsible panes) and
// correlate it to the right session.
//
// The wrapped Event is the sealed tagged union (KernelEvent.Kind()); the
// interaction codec (Phase 7, ADR-0018) serializes an Envelope with the Event
// nested as a discriminated union. Keeping the metadata in a wrapper — rather
// than as fields on every event variant — means a new event type needs no
// envelope boilerplate, and the relay layer sets SessionID/Depth in exactly one
// place.
type Envelope struct {
	SessionID SessionID   `json:"session_id"`
	Depth     int         `json:"depth"`
	Event     KernelEvent `json:"event"`
}
