package gateway

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/share"
	"github.com/unarbos/arbos/internal/sqlite"
)

// A write guest's chat_note frame is forwarded with the author stamped from the
// trusted cookie name, overwriting any client-supplied author (spoof blocked).
func TestFilterStampsChatNote(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"chat_note","data":{"text":"check the terminal","author":"Host"}}}`
	intent, keep := authorOf(t, frame, share.PermWrite, "Alice")
	if !keep {
		t.Fatal("write-guest chat_note was dropped")
	}
	n, ok := intent.(core.ChatNoteIntent)
	if !ok {
		t.Fatalf("expected ChatNoteIntent, got %T", intent)
	}
	if n.Author != "Alice" {
		t.Errorf("author = %q, want Alice (spoofed 'Host' must be overwritten)", n.Author)
	}
	if n.Text != "check the terminal" {
		t.Errorf("text mangled: %q", n.Text)
	}
}

// A read-only guest cannot post to the side chat — the frame is dropped before
// the kind switch by the perm gate.
func TestFilterReadGuestChatNoteDropped(t *testing.T) {
	frame := `{"type":"intent","intent":{"kind":"chat_note","data":{"text":"hi"}}}`
	if _, keep := filterShareFrame([]byte(frame), "s-1", share.PermRead, "Bob"); keep {
		t.Error("read-only guest chat_note should be dropped")
	}
}

// sessionReplay must emit a chat_note row (this is a Go type switch, NOT
// exhaustive-linted, so a missing case fails silently as "no chat history").
// The chat note must NOT appear as an agent-transcript row.
func TestSessionReplayIncludesChatNote(t *testing.T) {
	st, err := sqlite.Open(filepath.Join(t.TempDir(), "store.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = st.Close() }()
	ctx := context.Background()
	const sid core.SessionID = "s-replay"
	if err := st.CreateSession(ctx, core.Session{ID: sid}); err != nil {
		t.Fatal(err)
	}
	now := time.Unix(0, 0)
	if err := st.AppendEvent(ctx, core.NewMessageEvent(sid, core.Message{Role: core.RoleUser, Content: "agent prompt"}, now)); err != nil {
		t.Fatal(err)
	}
	if err := st.AppendEvent(ctx, core.NewChatNoteEvent(sid, core.Message{Role: core.RoleUser, Content: "side note", Author: "Alice"}, now)); err != nil {
		t.Fatal(err)
	}

	s := &Server{Store: st}
	out, err := s.sessionReplay(ctx, sid)
	if err != nil {
		t.Fatal(err)
	}
	events, _ := out["events"].([]replayJSON)
	var sawChatNote, sawUser bool
	for _, e := range events {
		switch e.Type {
		case "chat_note":
			sawChatNote = true
			if e.Text != "side note" || e.Author != "Alice" {
				t.Errorf("chat_note row wrong: %+v", e)
			}
		case "user":
			sawUser = true
			if e.Text == "side note" {
				t.Error("chat note leaked into the agent transcript as a user row")
			}
		}
	}
	if !sawChatNote {
		t.Error("sessionReplay did not emit a chat_note row — side chat would vanish on reload")
	}
	if !sawUser {
		t.Error("expected the agent user message to still replay")
	}
}
