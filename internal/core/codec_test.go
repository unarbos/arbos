package core_test

import (
	"encoding/json"
	"reflect"
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

// TestPayloadCodecRoundTrip is the golden the review asked for: every payload
// kind survives Encode -> Decode byte-for-byte. This is the contract the Phase-2
// SQLite store relies on — it persists EncodePayload bytes + Kind() and rebuilds
// via DecodePayload, so if this holds the store is a drop-in. Includes
// reasoning/tool-call fidelity on the assistant.
func TestPayloadCodecRoundTrip(t *testing.T) {
	cases := []struct {
		name string
		p    core.EventPayload
	}{
		{"user", core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "hi"}}},
		{"assistant_full_fidelity", core.MessagePayload{Message: core.Message{
			Role:      core.RoleAssistant,
			Content:   "answer",
			Reasoning: "because reasons",
			ToolCalls: []core.ToolCall{{ID: "t1", Name: "echo", Args: json.RawMessage(`{"text":"x"}`)}},
		}}},
		{"tool_result", core.ToolResultPayload{Result: core.ToolResult{CallID: "t1", Content: "r", IsError: false}}},
		{"usage", core.UsagePayload{Usage: core.Usage{PromptTokens: 1, CompletionTokens: 2, TotalTokens: 3}}},
		{"compressed", core.CompressionPayload{Summary: "older", ReplacedSeqLo: 1, ReplacedSeqHi: 5}},
		{"context", core.ContextPayload{Segments: []core.Segment{{Source: "memory", Content: "the user likes Go"}}}},
		{"interrupted", core.InterruptPayload{Reason: "user interrupt"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			data, err := core.EncodePayload(tc.p)
			if err != nil {
				t.Fatalf("encode: %v", err)
			}
			got, err := core.DecodePayload(tc.p.Kind(), data)
			if err != nil {
				t.Fatalf("decode: %v", err)
			}
			if got.Kind() != tc.p.Kind() {
				t.Fatalf("kind changed: %q -> %q", tc.p.Kind(), got.Kind())
			}
			if !reflect.DeepEqual(got, tc.p) {
				t.Fatalf("round-trip mismatch:\n got: %#v\nwant: %#v", got, tc.p)
			}
		})
	}
}

func TestEncodeNilPayloadErrors(t *testing.T) {
	if _, err := core.EncodePayload(nil); err == nil {
		t.Fatal("expected error encoding nil payload")
	}
}

func TestDecodeUnknownKindErrors(t *testing.T) {
	if _, err := core.DecodePayload(core.EventKind("totally-unknown"), []byte(`{}`)); err == nil {
		t.Fatal("expected error decoding unknown kind")
	}
}

// TestEveryEventKindDecodes guards the closed set: every EventKind the build
// declares must be decodable. Pairs with the exhaustive linter on DecodePayload
// so a new kind cannot be added without a decode path (and a malformed one is
// caught here rather than at runtime in the store).
func TestEveryEventKindDecodes(t *testing.T) {
	kinds := []core.EventKind{
		core.EventUserMessage,
		core.EventAssistantMessage,
		core.EventToolResult,
		core.EventUsage,
		core.EventCompressed,
		core.EventContext,
		core.EventInterrupted,
	}
	for _, k := range kinds {
		if _, err := core.DecodePayload(k, []byte(`{}`)); err != nil {
			t.Fatalf("kind %q must decode an empty object, got: %v", k, err)
		}
	}
}
