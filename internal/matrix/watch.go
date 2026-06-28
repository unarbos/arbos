package matrix

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/event"
)

// Watch runs the node agent's sync loop and calls fn for every text message
// posted into a session room the node knows about, decoded so the gateway never
// touches a Matrix type. It blocks until ctx is cancelled.
//
// foreign distinguishes the two origins of a room message (ADR-0041 "the room
// is the session"): an arbos-originated event carries the life.arbos metadata
// the mirror stamps (foreign=false) and is already on the browser's local
// stream, so re-surfacing it would echo; a message that lacks it (foreign=true)
// was typed from an external Matrix client logged in as the person, or sent by
// a federated peer — the genuinely new inbound the gateway forwards to the
// browser. The agent is a member of every session room, so one sync covers them
// all. History is skipped: only messages arriving after the watch starts are
// delivered (the agent's whole backlog would otherwise replay on first sync).
//
// A nil bridge or absent client is a no-op (the Matrix-off path).
func (b *Bridge) Watch(ctx context.Context, fn func(session core.SessionID, sender, body string, foreign bool)) error {
	if b == nil || b.agent == nil {
		return nil
	}
	// Take the current sync token first so the loop streams only what arrives
	// from now on, rather than replaying every room's whole timeline.
	resp, err := b.agent.SyncRequest(0, "", "", true, event.PresenceOnline, ctx)
	if err != nil {
		return err
	}
	b.agent.Store.SaveNextBatch(b.agent.UserID, resp.NextBatch)

	syncer := mautrix.NewDefaultSyncer()
	syncer.OnEventType(event.EventMessage, func(_ mautrix.EventSource, ev *event.Event) {
		sid, ok := b.SessionForRoom(ev.RoomID.String())
		if !ok {
			return // a room that backs no session we track
		}
		body, _ := ev.Content.Raw["body"].(string)
		if body == "" {
			return
		}
		_, arbosOriginated := ev.Content.Raw[ContentExt]
		fn(sid, ev.Sender.String(), body, !arbosOriginated)
	})
	b.agent.Syncer = syncer
	return b.agent.SyncWithContext(ctx)
}
