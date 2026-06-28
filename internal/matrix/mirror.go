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
//
// It holds both of the node's resident identities (ADR-0041 D9): the autonomous
// agent and the person operating it. A session room has both as members, and
// each event is published as whoever actually authored it — the person's
// messages and side chat come from the person, the agent's prose and tool work
// from the agent — so the Matrix sender is truthful (D4), which is what power
// levels and grants build on later. With a nil human client the mirror degrades
// to agent-only (the pre-identity path).
type clientMirror struct {
	agent   *mautrix.Client
	human   *mautrix.Client
	humanID id.UserID
	guests  *GuestRegistry      // shared-chat participants, keyed by (session, author); may be nil
	redact  func(string) string // outbound redaction twin (D12); nil = no scrub
}

// NewClientMirror builds a Mirror over the node's two resident clients plus the
// guest registry and the outbound redactor. human may be nil (agent-only) and
// humanID is the person's full user id used to invite them into each session
// room; guests may be nil (role-based routing only). redact, when set, is the
// anti-exfiltration twin (ADR-0016/0041 D12): it scrubs the COPY published to
// the room while the local log keeps the original — so a secret in a shared
// message or raised tool result never federates out, even as the agent's own
// trajectory stays intact. nil leaves the room copy verbatim.
func NewClientMirror(agent, human *mautrix.Client, humanID string, guests *GuestRegistry, redact func(string) string) Mirror {
	return &clientMirror{agent: agent, human: human, humanID: id.UserID(humanID), guests: guests, redact: redact}
}

func (m *clientMirror) CreateRoom(_ context.Context, sid core.SessionID) (string, error) {
	req := &mautrix.ReqCreateRoom{
		Name:       string(sid),
		Topic:      "arbos session " + string(sid),
		Preset:     "private_chat",
		Visibility: "private",
	}
	// Invite the person at creation so the room has both resident members.
	if m.human != nil && m.humanID != "" {
		req.Invite = []id.UserID{m.humanID}
	}
	resp, err := m.agent.CreateRoom(req)
	if err != nil {
		return "", err
	}
	// The person joins so the room is genuinely two-membered (the substrate for
	// presence, sender-correctness, and membership-based participation).
	// Best-effort: a join hiccup leaves them invited, still a real second member.
	if m.human != nil && m.humanID != "" {
		_, _ = m.human.JoinRoomByID(resp.RoomID)
	}
	return resp.RoomID.String(), nil
}

// SessionForRoom resolves a room to its session — the inverse of RoomForSession
// and the durable fallback the Store uses for the watcher's room->session
// routing when the in-memory map misses (a session created in a prior process
// and resumed here). The room's name IS the session id (CreateRoom sets it), so
// this is a single state lookup, not a scan. ok is false when the room has no
// such name (not an arbos session room).
func (m *clientMirror) SessionForRoom(_ context.Context, roomID string) (core.SessionID, bool) {
	var name event.RoomNameEventContent
	if err := m.agent.StateEvent(id.RoomID(roomID), event.StateRoomName, "", &name); err != nil {
		return "", false
	}
	if name.Name == "" {
		return "", false
	}
	return core.SessionID(name.Name), true
}

// RoomForSession resolves a session to its room by scanning the agent's joined
// rooms for the one named after the session (CreateRoom sets Name = session id).
// This is the durable fallback the Store uses when its in-memory map misses —
// a session created in a prior process and resumed here, whose room still lives
// on the homeserver. ok is false when no joined room carries that name.
func (m *clientMirror) RoomForSession(_ context.Context, sid core.SessionID) (string, bool) {
	resp, err := m.agent.JoinedRooms()
	if err != nil {
		return "", false
	}
	for _, rid := range resp.JoinedRooms {
		var name event.RoomNameEventContent
		if err := m.agent.StateEvent(rid, event.StateRoomName, "", &name); err != nil {
			continue
		}
		if name.Name == string(sid) {
			return rid.String(), true
		}
	}
	return "", false
}

func (m *clientMirror) Publish(_ context.Context, roomID string, e *core.Event) error {
	et, content := translate(e)
	// The redaction twin (D12): scrub the prose of the COPY going to the room,
	// never the local event (already stored). A federated secret can't be
	// unsent, so the guard runs here, at the one outbound seam.
	if m.redact != nil {
		if cm, ok := content.(map[string]any); ok {
			if body, ok := cm["body"].(string); ok {
				cm["body"] = m.redact(body)
			}
		}
	}
	_, err := m.senderFor(e).SendMessageEvent(id.RoomID(roomID), et, content)
	return err
}

// senderFor picks the client whose identity authored an event so the Matrix
// sender matches the real author rather than collapsing everyone into one
// account (D4/D9): a registered guest's own message/side-chat publishes as that
// guest; the person's as the person; everything else (the agent's prose, tool
// work) as the agent. The guest lookup is keyed by (session, author) — the
// author is the trusted display name the gateway stamped — and comes first so a
// shared-chat participant is never mistaken for the host. Falls back to the
// agent whenever the human client is absent.
func (m *clientMirror) senderFor(e *core.Event) *mautrix.Client {
	if author := eventAuthor(e); author != "" {
		if guest, ok := m.guests.client(e.SessionID, author); ok {
			return guest
		}
	}
	if m.human == nil {
		return m.agent
	}
	switch p := e.Payload.(type) {
	case core.MessagePayload:
		if p.Message.Role == core.RoleUser {
			return m.human
		}
	case core.ChatNotePayload:
		return m.human
	}
	return m.agent
}

// eventAuthor is the human display name attached to an event, if any — the
// trusted name the gateway stamped onto a participant's prompt or side chat.
// Empty for the host's own messages and for non-authored events.
func eventAuthor(e *core.Event) string {
	switch p := e.Payload.(type) {
	case core.MessagePayload:
		return p.Message.Author
	case core.ChatNotePayload:
		return p.Message.Author
	}
	return ""
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
