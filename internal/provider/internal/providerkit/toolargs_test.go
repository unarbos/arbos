package providerkit

import (
	"encoding/json"
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

// Calls must never emit a tool call whose Args is invalid JSON: a model that
// streams malformed arguments would otherwise crash the assistant-message
// persist (json.Marshal validates json.RawMessage) and abort the turn.
func TestCallsCoercesInvalidArgs(t *testing.T) {
	cases := map[string]struct {
		fragments []string
		wantArgs  string
	}{
		"valid passes through":     {[]string{`{"url":"https://x"}`}, `{"url":"https://x"}`},
		"empty becomes object":     {[]string{""}, `{}`},
		"missing comma coerced":    {[]string{`{"a":"b" "c":"d"}`}, `{}`},
		"unescaped quote coerced":  {[]string{`{"content":"he said "hi""}`}, `{}`},
		"truncated coerced":        {[]string{`{"path":"/tmp/x.md","content":"# Title`}, `{}`},
		"split valid fragments ok": {[]string{`{"u`, `rl":"`, `https://x"}`}, `{"url":"https://x"}`},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			acc := NewToolAccumulator()
			for _, f := range tc.fragments {
				acc.Add(0, "call-1", "fetch", f)
			}
			calls := acc.Calls()
			if len(calls) != 1 {
				t.Fatalf("got %d calls, want 1", len(calls))
			}
			if got := string(calls[0].Args); got != tc.wantArgs {
				t.Errorf("Args = %q, want %q", got, tc.wantArgs)
			}
			// The whole point: the resulting call must survive the persist path
			// (EncodePayload is what runs when the assistant message is appended).
			payload := core.MessagePayload{Message: core.Message{Role: core.RoleAssistant, ToolCalls: calls}}
			if _, err := core.EncodePayload(payload); err != nil {
				t.Errorf("assistant message with these args fails to encode (would abort the turn): %v", err)
			}
			if !json.Valid(calls[0].Args) {
				t.Errorf("Args is not valid JSON: %q", calls[0].Args)
			}
		})
	}
}
