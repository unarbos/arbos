package gateway

import (
	"encoding/json"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/share"
)

// authorOf runs a frame through the share filter and decodes the resulting
// intent's author. It fails the test if the frame was dropped.
func authorOf(t *testing.T, frame string, perm share.Perm, name string) (core.Intent, bool) {
	t.Helper()
	out, keep := filterShareFrame([]byte(frame), "s-1", perm, name)
	if !keep {
		return nil, false
	}
	var f struct {
		Intent json.RawMessage `json:"intent"`
	}
	if err := json.Unmarshal(out, &f); err != nil {
		t.Fatalf("filtered frame not JSON: %v", err)
	}
	intent, err := core.DecodeIntent(f.Intent)
	if err != nil {
		t.Fatalf("decode filtered intent: %v", err)
	}
	return intent, true
}

func TestFilterStampsGuestName(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"prompt","data":{"text":"hello"}}}`
	intent, keep := authorOf(t, frame, share.PermWrite, "Alice")
	if !keep {
		t.Fatal("write-perm prompt was dropped")
	}
	p, ok := intent.(core.PromptIntent)
	if !ok {
		t.Fatalf("expected PromptIntent, got %T", intent)
	}
	if p.Author != "Alice" {
		t.Errorf("author = %q, want Alice", p.Author)
	}
	if p.Text != "hello" {
		t.Errorf("text mangled: %q", p.Text)
	}
}

// The frame-filter must overwrite a client-supplied author so a guest cannot
// impersonate another participant.
func TestFilterOverwritesSpoofedAuthor(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"prompt","data":{"text":"hi","author":"Bob"}}}`
	intent, _ := authorOf(t, frame, share.PermWrite, "Alice")
	p := intent.(core.PromptIntent)
	if p.Author != "Alice" {
		t.Errorf("spoof not blocked: author = %q, want Alice", p.Author)
	}
}

func TestFilterSteerStampsName(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"steer","data":{"text":"wait"}}}`
	intent, keep := authorOf(t, frame, share.PermWrite, "Alice")
	if !keep {
		t.Fatal("steer dropped")
	}
	if s, ok := intent.(core.SteerIntent); !ok || s.Author != "Alice" {
		t.Errorf("steer author = %+v, want Alice", intent)
	}
}

// A full login (empty name) must pass the frame through unchanged.
func TestFilterEmptyNameNoStamp(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"prompt","data":{"text":"hello"}}}`
	intent, keep := authorOf(t, frame, share.PermWrite, "")
	if !keep {
		t.Fatal("prompt dropped")
	}
	if p := intent.(core.PromptIntent); p.Author != "" {
		t.Errorf("empty name should not stamp: author = %q", p.Author)
	}
}

func TestFilterReadPermDropsPrompt(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"prompt","data":{"text":"hello"}}}`
	if _, keep := filterShareFrame([]byte(frame), "s-1", share.PermRead, "Alice"); keep {
		t.Error("read-only guest prompt should be dropped")
	}
}

func TestSanitizeGuestName(t *testing.T) {
	cases := map[string]string{
		"  Alice  ":                      "Alice",
		"Bob\nInjection":                 "BobInjection",
		"":                               "",
		"\t\r":                           "",
		"012345678901234567890123456789012345": "01234567890123456789012345678901", // capped at 32 runes
	}
	for in, want := range cases {
		if got := sanitizeGuestName(in); got != want {
			t.Errorf("sanitizeGuestName(%q) = %q, want %q", in, got, want)
		}
	}
}
