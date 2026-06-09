package core

import "time"

// SourceMemory is the Segment.Source for the recalled-atoms block. It is
// load-bearing: the latest-per-source projection rule (see Project) replaces,
// rather than accumulates, the block for this source each turn — so a stable,
// unique source is what keeps the recalled memory from growing the prompt.
const SourceMemory = "memory"

// Atom is one durable comprehension — a note the agent writes to its future
// self. It is the whole memory data model: an ID (stable identity; reusing it
// is an update, so the curator merges by emitting the same ID), terse Content
// (the note itself; the subject is written *into* the prose so it can be found
// by association, never by an external scope key), and an update timestamp.
//
// Deliberately not modeled: workspace/session/user scope, kind, tags, vectors.
// Scope is emergent from what the note says and how well it matches the present
// moment at recall time — not a column. Everything the agent learns, across
// every session, lives in one flat set of atoms. See docs ADR on memory.
type Atom struct {
	ID        string
	Content   string
	UpdatedAt time.Time
}
