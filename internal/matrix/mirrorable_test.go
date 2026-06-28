package matrix

import (
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// mirrorable governs what reaches the room. A room-originated user message
// (injected to drive a turn) must NOT be re-published — it is already in the
// room — while ordinary prose still federates and private trajectory never does.
func TestMirrorableSkipsRoomOriginEcho(t *testing.T) {
	now := time.Unix(0, 0)
	const sid = core.SessionID("s")

	roomEcho := core.NewMessageEvent(sid, core.Message{
		Role:    core.RoleUser,
		Content: "hi from element",
		Origin:  core.OriginRoom,
	}, now)
	if mirrorable(roomEcho) {
		t.Error("a room-originated user message must not be mirrored back (would duplicate it in the room)")
	}

	localPrompt := core.NewMessageEvent(sid, core.Message{Role: core.RoleUser, Content: "hi from browser"}, now)
	if !mirrorable(localPrompt) {
		t.Error("an ordinary user message should federate to the room")
	}

	assistant := core.NewMessageEvent(sid, core.Message{Role: core.RoleAssistant, Content: "reply"}, now)
	if !mirrorable(assistant) {
		t.Error("an assistant message should federate to the room")
	}

	usage := core.NewUsageEvent(sid, core.Usage{}, now)
	if mirrorable(usage) {
		t.Error("a usage event is private trajectory and must stay local")
	}
}
