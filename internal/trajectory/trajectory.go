// Package trajectory is the one home for a session's machine-readable export
// shape. A trajectory is the lossless reproduction of a session — the event
// log, payloads verbatim — and its whole point is that a machine can read it
// back and reason about what the agent did ("was this the behavior we
// wanted?"). The CLI (`arbos export`) and the shareable trajectory link both
// emit through here so the two can never drift: a link an agent fetches is
// byte-identical to what the export command writes.
package trajectory

import (
	"encoding/json"
	"fmt"
	"io"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// EventLine is one full-fidelity JSONL record: a persisted event with its
// payload verbatim (reasoning, tool calls, tool results, usage, provider meta,
// config — everything the store holds). One line per event; this is the
// lossless trajectory shape a debugging agent replays and audits.
type EventLine struct {
	SessionID string          `json:"session_id"`
	Seq       int64           `json:"seq"`
	TurnID    int64           `json:"turn_id,omitempty"`
	Kind      string          `json:"kind"`
	Version   int             `json:"version"`
	CreatedAt time.Time       `json:"created_at"`
	Payload   json.RawMessage `json:"payload"`
}

// MessagesLine is one session rendered through core.Project — the exact
// conversation the provider saw, system prompt and injected context included
// (read from the log's config events, not the current binary). One line per
// session, training-sample shaped.
type MessagesLine struct {
	SessionID string            `json:"session_id"`
	Model     string            `json:"model,omitempty"`
	Tools     []core.ToolSchema `json:"tools,omitempty"`
	Messages  []core.Message    `json:"messages"`
}

// WriteEvents encodes a session's events as full-fidelity JSONL, one event per
// line, in log order. This is the lossless shape — the default a machine reads.
func WriteEvents(enc *json.Encoder, id core.SessionID, events []core.Event) error {
	for i := range events {
		payload, err := core.EncodePayload(events[i].Payload)
		if err != nil {
			return fmt.Errorf("trajectory %s: encode seq %d: %w", id, events[i].Seq, err)
		}
		err = enc.Encode(EventLine{
			SessionID: string(id),
			Seq:       events[i].Seq,
			TurnID:    events[i].TurnID,
			Kind:      string(events[i].Payload.Kind()),
			Version:   events[i].Version,
			CreatedAt: events[i].CreatedAt.UTC(),
			Payload:   payload,
		})
		if err != nil {
			return err
		}
	}
	return nil
}

// WriteMessages encodes a session as a single projected-conversation line: the
// exact messages the provider saw. The config events in the log are the
// authority for the model, tools, and system prompt; fallbackModel is used only
// when the log predates config logging.
func WriteMessages(enc *json.Encoder, id core.SessionID, events []core.Event, fallbackModel string) error {
	line := MessagesLine{SessionID: string(id)}
	if cfg, ok := core.LatestConfig(events); ok {
		line.Model = cfg.Model
		line.Tools = cfg.Tools
		line.Messages = core.Project(events, cfg.SystemPrompt)
	} else {
		line.Model = fallbackModel
		line.Messages = core.Project(events, "")
	}
	return enc.Encode(line)
}

// NewEncoder returns a JSON encoder configured for trajectory output: one
// object per line, with HTML escaping off so the record's own string fields
// keep `<`, `>`, `&` literal. (Event payloads are already-encoded RawMessage
// the kernel escaped at store time; either way every line is valid JSON a
// parser reads back transparently.)
func NewEncoder(w io.Writer) *json.Encoder {
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	return enc
}
