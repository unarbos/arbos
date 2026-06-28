package gateway

import (
	"context"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
)

// A message arriving from an external Matrix client into a CLOSED session wakes
// it: runRoomTurn attaches, drives a turn, and the user prompt lands carrying
// OriginRoom (so the mirror won't re-publish the room's own copy) while the
// agent produces a reply — proving "talk to your agent from any Matrix client"
// reaches a chat nobody has open.
func TestRunRoomTurnWakesClosedSession(t *testing.T) {
	store := fake.NewStore()
	eng := engine.New(fake.Provider{}, fake.Tools{}, store, fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	s := &Server{Engine: eng}

	const sid = core.SessionID("s-room-wake")
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	// Synchronous (the production caller wraps this in a goroutine): it returns
	// when the turn reaches its terminal event.
	s.runRoomTurn(ctx, sid, "@human:localhost", "hello agent",
		core.PromptIntent{Text: "hello agent", Origin: core.OriginRoom, Author: "human"})

	evs, err := store.Events(ctx, sid)
	if err != nil {
		t.Fatalf("Events: %v", err)
	}
	var sawRoomPrompt, sawAssistant bool
	for _, ev := range evs {
		if m, ok := ev.Payload.(core.MessagePayload); ok {
			switch m.Message.Role {
			case core.RoleUser:
				if m.Message.Content == "hello agent" && m.Message.Origin == core.OriginRoom {
					sawRoomPrompt = true
				}
			case core.RoleAssistant:
				sawAssistant = true
			}
		}
	}
	if !sawRoomPrompt {
		t.Error("the external message did not land as a user prompt carrying OriginRoom")
	}
	if !sawAssistant {
		t.Error("the woken session produced no agent reply")
	}
}
