package core

import (
	"testing"
	"time"
)

// A ChatNoteIntent round-trips through the {kind,data} codec — DecodeIntent
// returns a hard error on a missing case, so this proves the switch is wired.
func TestChatNoteIntentRoundTrip(t *testing.T) {
	in := ChatNoteIntent{Text: "check the terminal tab", Author: "Alice", Origin: "web:7"}
	b, err := EncodeIntent(in)
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	got, err := DecodeIntent(b)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	cn, ok := got.(ChatNoteIntent)
	if !ok {
		t.Fatalf("decoded to %T, want ChatNoteIntent", got)
	}
	if cn.Text != in.Text || cn.Author != in.Author || cn.Origin != in.Origin {
		t.Errorf("round-trip mismatch: %+v vs %+v", cn, in)
	}
}

// The live ChatNote kernel event round-trips through DecodeEvent.
func TestChatNoteEventRoundTrip(t *testing.T) {
	in := ChatNote{Text: "hi", Author: "Bob", Origin: "web:3", TS: 1718553600000}
	b, err := EncodeEvent(in)
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	got, err := DecodeEvent(b)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	cn, ok := got.(ChatNote)
	if !ok {
		t.Fatalf("decoded to %T, want ChatNote", got)
	}
	if cn.Text != in.Text || cn.Author != in.Author || cn.Origin != in.Origin || cn.TS != in.TS {
		t.Errorf("round-trip mismatch: %+v vs %+v", cn, in)
	}
}

// The persisted ChatNotePayload round-trips through the kind-discriminated
// payload codec (which IS exhaustive-linted).
func TestChatNotePayloadRoundTrip(t *testing.T) {
	p := ChatNotePayload{Message: Message{Role: RoleUser, Content: "note", Author: "Alice"}}
	b, err := EncodePayload(p)
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	got, err := DecodePayload(EventChatNote, b)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if _, ok := got.(ChatNotePayload); !ok {
		t.Fatalf("decoded to %T, want ChatNotePayload", got)
	}
}

// A chat note MUST NOT project to a model message — the single isolation point.
func TestChatNoteNotProjected(t *testing.T) {
	ev := NewChatNoteEvent("s-1", Message{Role: RoleUser, Content: "side comms", Author: "Alice"}, time.Unix(0, 0))
	if _, ok := ProjectEvent(*ev); ok {
		t.Error("ProjectEvent returned a message for a chat note — it would leak into the model context")
	}
	// And it contributes nothing to a full projection.
	msgs := ProjectConversation([]Event{*ev})
	if len(msgs) != 0 {
		t.Errorf("ProjectConversation yielded %d messages for a chat-note-only log, want 0", len(msgs))
	}
}

// A ChatNotePayload with Role=RoleUser passes Validate (its check is scoped to
// MessagePayload, not this distinct type).
func TestChatNotePayloadValidates(t *testing.T) {
	ev := NewChatNoteEvent("s-1", Message{Role: RoleUser, Content: "ok"}, time.Unix(0, 0))
	if err := ev.Validate(); err != nil {
		t.Errorf("Validate rejected a chat note: %v", err)
	}
}
