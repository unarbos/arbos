package engine

import (
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/fake"
)

// projectUserContent runs the read-side projection over a single user message
// and returns the content the provider would see for it.
func projectUserContent(t *testing.T, cfg Config, m core.Message) string {
	t.Helper()
	e := New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(), cfg)
	ev := core.NewMessageEvent("s-project", m, time.Unix(0, 0))
	msgs := e.project([]core.Event{*ev})
	for _, msg := range msgs {
		if msg.Role == core.RoleUser {
			return msg.Content
		}
	}
	t.Fatalf("no user message in projection of %+v", m)
	return ""
}

func TestProjectionPrefixesAuthor(t *testing.T) {
	got := projectUserContent(t, Config{}, core.Message{
		Role:    core.RoleUser,
		Content: "ship it",
		Author:  "Alice",
	})
	if want := "Alice: ship it"; got != want {
		t.Errorf("author prefix: got %q, want %q", got, want)
	}
}

func TestProjectionNoAuthorUnchanged(t *testing.T) {
	got := projectUserContent(t, Config{}, core.Message{
		Role:    core.RoleUser,
		Content: "ship it",
	})
	if want := "ship it"; got != want {
		t.Errorf("no-author projection should be unchanged: got %q, want %q", got, want)
	}
}

// The author prefix is applied AFTER ExpandUser, so a guest's slash command
// still expands and the model still sees who sent it.
func TestProjectionAuthorAfterExpand(t *testing.T) {
	cfg := Config{ExpandUser: func(s string) string {
		if s == "/deploy" {
			return "Please run the deploy checklist."
		}
		return s
	}}
	got := projectUserContent(t, cfg, core.Message{
		Role:    core.RoleUser,
		Content: "/deploy",
		Author:  "Bob",
	})
	if want := "Bob: Please run the deploy checklist."; got != want {
		t.Errorf("expand-then-prefix: got %q, want %q", got, want)
	}
}
