package core

import (
	"encoding/json"
	"fmt"
)

// EncodePayload serializes an event payload to JSON. The concrete payload's
// fields are written directly; the discriminator is Kind(), which the
// SessionStore persists as a separate column. See ADR-0008.
func EncodePayload(p EventPayload) ([]byte, error) {
	if p == nil {
		return nil, fmt.Errorf("encode payload: nil payload")
	}
	return json.Marshal(p)
}

// DecodePayload reconstructs a payload from its kind discriminator and JSON
// bytes — the inverse of EncodePayload. It lives in core, next to the kinds and
// constructors, so the closed set of payload shapes has exactly ONE home: a
// store that grows a new backend (SQLite, Phase 2) calls this and never
// re-enumerates payloads, keeping payload knowledge from leaking across the
// boundary (ADR-0008). The switch is over EventKind so the `exhaustive` linter
// enforces that every kind is handled (ADR-0010).
func DecodePayload(kind EventKind, data []byte) (EventPayload, error) {
	switch kind {
	case EventUserMessage, EventAssistantMessage:
		return unmarshalPayload[MessagePayload](kind, data)
	case EventToolResult:
		return unmarshalPayload[ToolResultPayload](kind, data)
	case EventUsage:
		return unmarshalPayload[UsagePayload](kind, data)
	case EventCompressed:
		return unmarshalPayload[CompressionPayload](kind, data)
	case EventContext:
		return unmarshalPayload[ContextPayload](kind, data)
	case EventInterrupted:
		return unmarshalPayload[InterruptPayload](kind, data)
	case EventConfig:
		return unmarshalPayload[ConfigPayload](kind, data)
	}
	// Unknown discriminator: a newer log read by an older binary. The caller
	// (loader) decides whether to skip or fail; we do not guess. See ADR-0010.
	return nil, fmt.Errorf("decode: unknown event kind %q", kind)
}

func unmarshalPayload[T EventPayload](kind EventKind, data []byte) (EventPayload, error) {
	var p T
	if err := json.Unmarshal(data, &p); err != nil {
		return nil, fmt.Errorf("decode %s: %w", kind, err)
	}
	return p, nil
}
