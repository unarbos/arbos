package core

import (
	"encoding/json"
	"fmt"
)

// This file is the interaction codec: tagged-union JSON encode/decode for the
// kernel's wire vocabulary — Intent (in), KernelEvent (out), and the Envelope
// that wraps emitted events. It mirrors the EventPayload codec but for the
// live protocol rather than the persisted log, and is the single home for that
// mapping so the Phase-7 control seam (and any out-of-process frontend: web,
// desktop, remote arbos) never re-enumerates the variant set. The switches are
// over the Kind discriminators so the `exhaustive` linter forces every variant
// to be handled. See ADR-0018.

type wireFrame struct {
	Kind string          `json:"kind"`
	Data json.RawMessage `json:"data"`
}

// EncodeIntent serializes an Intent to a {kind,data} frame.
func EncodeIntent(i Intent) ([]byte, error) {
	data, err := json.Marshal(i)
	if err != nil {
		return nil, fmt.Errorf("encode intent %s: %w", i.Kind(), err)
	}
	return json.Marshal(wireFrame{Kind: string(i.Kind()), Data: data})
}

// DecodeIntent reconstructs an Intent from a {kind,data} frame.
func DecodeIntent(b []byte) (Intent, error) {
	var f wireFrame
	if err := json.Unmarshal(b, &f); err != nil {
		return nil, fmt.Errorf("decode intent frame: %w", err)
	}
	switch IntentKind(f.Kind) {
	case IntentPrompt:
		return unmarshalInto[PromptIntent](f)
	case IntentInterrupt:
		return unmarshalInto[InterruptIntent](f)
	case IntentResume:
		return unmarshalInto[ResumeIntent](f)
	case IntentApprovalResponse:
		return unmarshalInto[ApprovalResponseIntent](f)
	case IntentSetModel:
		return unmarshalInto[SetModelIntent](f)
	case IntentCompact:
		return unmarshalInto[CompactIntent](f)
	}
	return nil, fmt.Errorf("decode intent: unknown kind %q", f.Kind)
}

// EncodeEvent serializes a KernelEvent to a {kind,data} frame.
func EncodeEvent(e KernelEvent) ([]byte, error) {
	data, err := json.Marshal(e)
	if err != nil {
		return nil, fmt.Errorf("encode event %s: %w", e.Kind(), err)
	}
	return json.Marshal(wireFrame{Kind: string(e.Kind()), Data: data})
}

// DecodeEvent reconstructs a KernelEvent from a {kind,data} frame.
func DecodeEvent(b []byte) (KernelEvent, error) {
	var f wireFrame
	if err := json.Unmarshal(b, &f); err != nil {
		return nil, fmt.Errorf("decode event frame: %w", err)
	}
	switch KernelEventKind(f.Kind) {
	case KernelEventMessageDelta:
		return unmarshalEvent[MessageDelta](f)
	case KernelEventReasoningDelta:
		return unmarshalEvent[ReasoningDelta](f)
	case KernelEventToolStarted:
		return unmarshalEvent[ToolStarted](f)
	case KernelEventToolFinished:
		return unmarshalEvent[ToolFinished](f)
	case KernelEventTurnComplete:
		return unmarshalEvent[TurnComplete](f)
	case KernelEventInterrupted:
		return unmarshalEvent[Interrupted](f)
	case KernelEventError:
		return unmarshalEvent[ErrorEvent](f)
	case KernelEventQueued:
		return unmarshalEvent[Queued](f)
	case KernelEventApprovalRequest:
		return unmarshalEvent[ApprovalRequest](f)
	}
	return nil, fmt.Errorf("decode event: unknown kind %q", f.Kind)
}

// wireEnvelope is the on-the-wire form of an Envelope: the event is itself a
// tagged frame so the whole thing round-trips through one JSON document.
type wireEnvelope struct {
	SessionID SessionID       `json:"session_id"`
	Depth     int             `json:"depth"`
	Event     json.RawMessage `json:"event"`
}

// EncodeEnvelope serializes an Envelope (the unit the engine emits) to JSON.
func EncodeEnvelope(e Envelope) ([]byte, error) {
	ev, err := EncodeEvent(e.Event)
	if err != nil {
		return nil, err
	}
	return json.Marshal(wireEnvelope{SessionID: e.SessionID, Depth: e.Depth, Event: ev})
}

// DecodeEnvelope reconstructs an Envelope from JSON.
func DecodeEnvelope(b []byte) (Envelope, error) {
	var we wireEnvelope
	if err := json.Unmarshal(b, &we); err != nil {
		return Envelope{}, fmt.Errorf("decode envelope: %w", err)
	}
	ev, err := DecodeEvent(we.Event)
	if err != nil {
		return Envelope{}, err
	}
	return Envelope{SessionID: we.SessionID, Depth: we.Depth, Event: ev}, nil
}

func unmarshalInto[T Intent](f wireFrame) (Intent, error) {
	var v T
	if len(f.Data) > 0 {
		if err := json.Unmarshal(f.Data, &v); err != nil {
			return nil, fmt.Errorf("decode intent %s: %w", f.Kind, err)
		}
	}
	return v, nil
}

func unmarshalEvent[T KernelEvent](f wireFrame) (KernelEvent, error) {
	var v T
	if len(f.Data) > 0 {
		if err := json.Unmarshal(f.Data, &v); err != nil {
			return nil, fmt.Errorf("decode event %s: %w", f.Kind, err)
		}
	}
	return v, nil
}
