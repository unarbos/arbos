package core_test

import (
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

func TestIntentCodecRoundTrip(t *testing.T) {
	cases := []core.Intent{
		core.PromptIntent{Text: "hi"},
		core.InterruptIntent{},
		core.ResumeIntent{SessionID: "s1"},
		core.ApprovalResponseIntent{RequestID: "r1", Approved: true, Reason: "ok"},
		core.SetModelIntent{Model: "m1"},
		core.CompactIntent{},
	}
	for _, want := range cases {
		b, err := core.EncodeIntent(want)
		if err != nil {
			t.Fatalf("encode %T: %v", want, err)
		}
		got, err := core.DecodeIntent(b)
		if err != nil {
			t.Fatalf("decode %T: %v", want, err)
		}
		if got.Kind() != want.Kind() {
			t.Fatalf("kind mismatch: got %q want %q", got.Kind(), want.Kind())
		}
		if got != want {
			t.Fatalf("round-trip mismatch: got %#v want %#v", got, want)
		}
	}
}

func TestEventCodecRoundTrip(t *testing.T) {
	cases := []core.KernelEvent{
		core.MessageDelta{Text: "x"},
		core.ReasoningDelta{Text: "y"},
		core.ToolStarted{Call: core.ToolCall{ID: "1", Name: "echo"}},
		core.ToolFinished{Result: core.ToolResult{CallID: "1", Content: "ok"}},
		core.TurnComplete{FinalResponse: "done", StopReason: core.StopAnswered, Usage: core.Usage{TotalTokens: 3}},
		core.Interrupted{},
		core.ErrorEvent{Category: core.ErrProvider, Retryable: true, Err: "boom"},
		core.Queued{Text: "later"},
		core.ApprovalRequest{RequestID: "r", Call: core.ToolCall{Name: "rm"}, Reason: "danger"},
	}
	for _, want := range cases {
		b, err := core.EncodeEvent(want)
		if err != nil {
			t.Fatalf("encode %T: %v", want, err)
		}
		got, err := core.DecodeEvent(b)
		if err != nil {
			t.Fatalf("decode %T: %v", want, err)
		}
		if got.Kind() != want.Kind() {
			t.Fatalf("kind mismatch: got %q want %q", got.Kind(), want.Kind())
		}
	}
}

func TestEnvelopeCodecRoundTrip(t *testing.T) {
	env := core.Envelope{
		SessionID: "s9",
		Depth:     2,
		Event:     core.TurnComplete{FinalResponse: "hi", StopReason: core.StopAnswered},
	}
	b, err := core.EncodeEnvelope(env)
	if err != nil {
		t.Fatal(err)
	}
	got, err := core.DecodeEnvelope(b)
	if err != nil {
		t.Fatal(err)
	}
	if got.SessionID != "s9" || got.Depth != 2 || got.Event.Kind() != core.KernelEventTurnComplete {
		t.Fatalf("envelope round-trip mismatch: %#v", got)
	}
}

func TestDecodeUnknownKinds(t *testing.T) {
	if _, err := core.DecodeIntent([]byte(`{"kind":"nope","data":{}}`)); err == nil {
		t.Fatal("expected error for unknown intent kind")
	}
	if _, err := core.DecodeEvent([]byte(`{"kind":"nope","data":{}}`)); err == nil {
		t.Fatal("expected error for unknown event kind")
	}
}
