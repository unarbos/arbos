package matrix

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// clientMirror is the mautrix-backed Mirror: it creates a room per session and
// publishes Room-audience events into it. This is the only Store file that
// imports a Matrix client type; the Store itself stays Matrix-free.
type clientMirror struct {
	client *mautrix.Client
}

// NewClientMirror builds a Mirror over a logged-in mautrix client (the agent's
// own user, talking to its loopback homeserver).
func NewClientMirror(client *mautrix.Client) Mirror {
	return &clientMirror{client: client}
}

func (m *clientMirror) CreateRoom(_ context.Context, sid core.SessionID) (string, error) {
	resp, err := m.client.CreateRoom(&mautrix.ReqCreateRoom{
		Name:       string(sid),
		Topic:      "arbos session " + string(sid),
		Preset:     "private_chat",
		Visibility: "private",
	})
	if err != nil {
		return "", err
	}
	return resp.RoomID.String(), nil
}

func (m *clientMirror) Publish(_ context.Context, roomID string, e *core.Event) error {
	et, content := translate(e)
	_, err := m.client.SendMessageEvent(id.RoomID(roomID), et, content)
	return err
}

// translate maps a kernel event to a Matrix room event (D10): prose rides
// m.room.message with the arbos metadata under the life.arbos content key;
// everything else is a life.arbos.* custom type. The metadata carries turn_id /
// kind / seq so an arbos client can correlate, while a generic client (Element)
// still renders the prose body.
func translate(e *core.Event) (event.Type, any) {
	meta := map[string]any{
		"turn_id": e.TurnID,
		"kind":    string(e.Payload.Kind()),
		"seq":     e.Seq,
	}
	content := map[string]any{ContentExt: meta}
	switch p := e.Payload.(type) {
	case core.MessagePayload:
		content["msgtype"] = "m.text"
		content["body"] = p.Message.Content
		meta["role"] = string(p.Message.Role)
		return event.EventMessage, content
	case core.ChatNotePayload:
		content["body"] = p.Message.Content
		meta["author"] = p.Message.Author
		return event.NewEventType(EvtChatNote), content
	case core.ToolResultPayload:
		content["call_id"] = p.Result.CallID
		content["body"] = p.Result.Content
		content["is_error"] = p.Result.IsError
		return event.NewEventType(EvtToolResult), content
	default:
		content["body"] = "(" + string(e.Payload.Kind()) + ")"
		return event.NewEventType("life.arbos." + string(e.Payload.Kind())), content
	}
}
