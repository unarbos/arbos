// Package outbox is the agent's voice when no conversation is open: durable
// messages to the user, delivered into the conversation that created them. It
// completes the quartet:
//
//	STREAM  what happened          — the event log
//	ATOMS   what the agent knows   — internal/mind
//	PLAN    what the agent intends — internal/plan
//	OUTBOX  what you must hear     — here
//
// A turn's reply reaches only the conversation that prompted it; everything
// else the user must hear — a scheduled reminder firing, finished background
// work, an escalation — becomes a row here, stamped with its originating
// session. Delivery is claim-then-deliver and session-scoped: a door
// atomically claims its own conversations' undelivered rows before showing
// them, so the same message is never told twice and a notice never spills
// into a chat that did not create it. The notice waits, durably, for its own
// conversation to reopen (broadcast-class rows, which have no conversation,
// are the sole exception and reach any door). At-most-once on purpose — for
// speech, a rare lost line beats ever being double-pinged.
package outbox

import (
	"context"
	"fmt"
	"reflect"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/codingspec"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// ViaTerminal is the delivered_via marker for the interactive terminal door.
// Future doors record their own ("telegram:chat/123", "brief").
const ViaTerminal = "terminal"

// Message is one undelivered (or just-claimed) line for the user.
type Message struct {
	ID        int64
	Text      string
	Session   string // originating session
	CreatedAt time.Time
}

// BroadcastSessions are session markers with no chat to route to — rows from
// before session scoping, and kernel-claimed mechanical nodes that never
// recorded an origin. Doors deliver these everywhere; everything else routes
// to the conversation that created the work.
var BroadcastSessions = []string{"", "kernel"}

// IsBroadcast reports whether a message's session marker is broadcast-class.
func IsBroadcast(session string) bool {
	for _, b := range BroadcastSessions {
		if session == b {
			return true
		}
	}
	return false
}

// Store is the storage the outbox needs, satisfied by *sqlite.Store. Narrow on
// purpose (the mind.Store / plan.Store pattern).
type Store interface {
	// Notify appends one message for the user.
	Notify(ctx context.Context, text, session string) error
}

// Args are the arguments to the notify tool.
type Args struct {
	Message string `json:"message" desc:"What the user must hear. Short and self-contained — it may arrive on a phone."`
}

// RegisterTool adds the notify tool to a registry. Reflected at registration
// like delegate and plan (the documented ADR-0004 carve-out for dynamically
// wired tools).
func RegisterTool(reg *tool.Registry, store Store) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(Args{}))
	if err != nil {
		return fmt.Errorf("notify schema: %w", err)
	}
	spec := tool.NewSpec("notify",
		"Send the user a durable message for when your turn's reply won't reach them on its own — a scheduled node firing, background or delegate work finishing, a mission blocked on them. It is delivered into the conversation the work belongs to (spawned sessions resolve to the chat that started them) and surfaces the next time that conversation is open; it never spills into an unrelated chat. Never use it as a substitute for replying in conversation.",
		false,
		func(ctx context.Context, a Args) (string, error) {
			text := strings.TrimSpace(a.Message)
			if text == "" {
				return "", fmt.Errorf("notify: message must not be empty")
			}
			c, _ := obs.From(ctx)
			if err := store.Notify(ctx, text, c.SessionID); err != nil {
				return "", err
			}
			return "Queued for this conversation; it surfaces the next time its door is open.", nil
		})
	return reg.Register(spec, schema)
}

// PromptInfo is the notify tool's system-prompt metadata.
func PromptInfo() codingspec.ToolPromptInfo {
	return codingspec.ToolPromptInfo{
		Name:    "notify",
		Snippet: "Send the user a durable message that surfaces in the conversation the work belongs to (scheduled reminders, finished background work)",
		Guidelines: []string{
			"Your replies reach only the conversation that prompted them. When the user must hear something your turn's reply won't carry — a scheduled reminder firing, background work finishing, a blocked mission needing them — send it with notify; it lands in the conversation the work belongs to, the next time that door is open.",
			"Notify sparingly and concretely: one short message that stands alone, only when it genuinely concerns the user.",
		},
	}
}
