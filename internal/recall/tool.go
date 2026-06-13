// Package recall gives the agent a window onto its other sessions: the user's
// past chats and the agent's own autonomous runs (scheduler / Ralph-loop
// iterations). It is the seam behind a request like "what do you think about
// the results from that Ralph loop I just ran" — the agent lists recent
// sessions to find the one meant, then hands it to a reader sub-agent that
// studies the full transcript and returns a focused digest. The big transcript
// lands in the reader's context, never the calling chat's, so reviewing a long
// run costs a summary instead of the whole log.
package recall

import (
	"context"
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// SessionInfo is one row of the cross-session index: enough for the agent to
// recognize a session by what it was without loading its transcript.
type SessionInfo struct {
	ID        core.SessionID
	Kind      string // "chat" (a human conversation) or "run" (autonomous work)
	Title     string // first user prompt, trimmed — a run's is its goal
	Status    core.SessionStatus
	UpdatedAt time.Time
}

// Store is the read side of the durable session log the sessions tool needs:
// list recent sessions and read one's event log. *sqlite.Store satisfies it
// (adapted at wiring); a fake backs tests. Narrow on purpose — the plan.Store /
// outbox.Store pattern.
type Store interface {
	RecentSessions(ctx context.Context, limit int) ([]SessionInfo, error)
	Events(ctx context.Context, id core.SessionID) ([]core.Event, error)
}

// Reader runs a one-shot sub-agent with the given instruction and returns its
// final text plus the child session id (for the UI to reopen). recall uses it
// to hand a transcript to a fresh reader whose digest — not the transcript —
// returns to the calling chat. It mirrors the delegate tool's spawn path.
type Reader func(ctx context.Context, instruction string) (text, childID string, err error)

// Args are the arguments to the sessions tool.
type Args struct {
	Op    string   `json:"op" desc:"list or review."`
	Limit int      `json:"limit,omitempty" desc:"list: max rows (default 20)."`
	IDs   []string `json:"ids,omitempty" desc:"review: one or more session ids from a list result, read together — e.g. every iteration of one loop."`
	Focus string   `json:"focus,omitempty" desc:"review: what you want to learn from those sessions — the question the reader should answer."`
}

const (
	defaultListLimit = 20
	// perSessionChars bounds one transcript so a single huge session cannot
	// crowd out the others in a multi-session review; totalReviewChars bounds
	// the whole hand-off to the reader.
	perSessionChars  = 40000
	totalReviewChars = 120000
)

// RegisterTool adds the sessions tool to a registry. Like delegate and plan it
// is registered dynamically by whoever wires the store and a Reader, so its
// schema is reflected once here with the generator's reflector.
func RegisterTool(reg *tool.Registry, store Store, read Reader) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(Args{}))
	if err != nil {
		return fmt.Errorf("sessions schema: %w", err)
	}
	spec := tool.NewRichSpec("sessions",
		"Look across your other sessions and pull their context in without bloating this chat. op:list shows recent sessions newest first — the user's other chats and your own autonomous runs (scheduler / Ralph-loop iterations), each labeled by what it was. op:review hands one or more of those sessions (by id) to a reader sub-agent that studies their full transcripts and returns a focused digest answering your `focus` question — so \"what happened in that Ralph loop I just ran\" costs you a summary, not the whole log. Reach for it whenever the user refers to work done in another run or conversation.",
		false,
		func(ctx context.Context, a Args) (tool.Result, error) {
			switch a.Op {
			case "list":
				s, err := listOp(ctx, store, a)
				return tool.Result{Content: s}, err
			case "review":
				return reviewOp(ctx, store, read, a)
			}
			return tool.Result{}, fmt.Errorf("sessions: unknown op %q (use list or review)", a.Op)
		})
	// Mirrors delegate: a review spawns a sub-agent (not a pure read), and the
	// empty footprint lets reviews fan out in parallel with everything else.
	spec = tool.WithAccess(spec, func(Args) core.AccessSet {
		return core.AccessSet{}
	})
	return reg.Register(spec, schema)
}

func listOp(ctx context.Context, store Store, a Args) (string, error) {
	limit := a.Limit
	if limit <= 0 {
		limit = defaultListLimit
	}
	rows, err := store.RecentSessions(ctx, limit)
	if err != nil {
		return "", err
	}
	if len(rows) == 0 {
		return "No other sessions yet.", nil
	}
	now := time.Now()
	var b strings.Builder
	fmt.Fprintf(&b, "%d recent sessions, newest first:\n\n", len(rows))
	for _, r := range rows {
		fmt.Fprintf(&b, "- [%s] %s — %s (%s ago", r.Kind, r.ID, r.Title, ago(now.Sub(r.UpdatedAt)))
		if r.Status == core.SessionActive {
			b.WriteString(", active")
		}
		b.WriteString(")\n")
	}
	b.WriteString("\nReview one or more with op:review, ids:[…], focus:\"…\".")
	return b.String(), nil
}

func reviewOp(ctx context.Context, store Store, read Reader, a Args) (tool.Result, error) {
	if len(a.IDs) == 0 {
		return tool.Result{}, fmt.Errorf("sessions review: ids is required (list first to find them)")
	}
	if read == nil {
		return tool.Result{}, fmt.Errorf("sessions review: no reader configured on this host")
	}
	focus := strings.TrimSpace(a.Focus)
	if focus == "" {
		focus = "Summarize what happened and the key results."
	}

	var b strings.Builder
	var loaded int
	for _, id := range a.IDs {
		// Cap each transcript by the budget left, so the whole hand-off stays
		// under totalReviewChars instead of overshooting by up to one
		// perSessionChars (a soft cap the constant's name would belie).
		budget := totalReviewChars - b.Len()
		if budget <= 0 {
			fmt.Fprintf(&b, "\n[reached the review size limit; %d of %d sessions included]\n", loaded, len(a.IDs))
			break
		}
		if budget > perSessionChars {
			budget = perSessionChars
		}
		events, err := store.Events(ctx, core.SessionID(id))
		if err != nil {
			return tool.Result{}, fmt.Errorf("sessions review: read %s: %w", id, err)
		}
		if len(events) == 0 {
			fmt.Fprintf(&b, "===== session %s =====\n(no such session, or it never ran)\n\n", id)
			continue
		}
		fmt.Fprintf(&b, "===== session %s =====\n%s\n\n", id, renderTranscript(events, budget))
		loaded++
	}
	if loaded == 0 {
		return tool.Result{}, fmt.Errorf("sessions review: none of the given ids name a session with a transcript")
	}

	instruction := fmt.Sprintf(
		"You are a reader sub-agent. Below are transcripts from other arbos sessions. Study them and answer the question for the calling agent: report findings concisely, quote the concrete details that matter (commands, outcomes, numbers, errors), and call out anything that looks wrong. Do not run tools — everything you need is in the transcripts.\n\nQuestion: %s\n\n%s",
		focus, strings.TrimSpace(b.String()))

	text, childID, err := read(ctx, instruction)
	if err != nil {
		return tool.Result{}, fmt.Errorf("sessions review: reader failed: %w", err)
	}
	details, _ := json.Marshal(struct {
		ChildSession string `json:"childSession"`
	}{childID})
	return tool.Result{Content: text, Details: details}, nil
}

// renderTranscript folds a session's event log into plain text for a reader,
// reusing the same conversation projection the engine feeds the model, capped
// so one session cannot dominate a review.
func renderTranscript(events []core.Event, maxChars int) string {
	msgs := core.ProjectConversation(events)
	var b strings.Builder
	for _, m := range msgs {
		switch m.Role {
		case core.RoleUser:
			b.WriteString("USER:\n")
		case core.RoleAssistant:
			b.WriteString("ASSISTANT:\n")
		case core.RoleTool:
			b.WriteString("TOOL RESULT:\n")
		default:
			b.WriteString("(summary):\n")
		}
		if c := strings.TrimSpace(m.Content); c != "" {
			b.WriteString(c)
			b.WriteByte('\n')
		}
		for _, tc := range m.ToolCalls {
			fmt.Fprintf(&b, "[calls %s %s]\n", tc.Name, string(tc.Args))
		}
		b.WriteByte('\n')
	}
	out := strings.TrimSpace(b.String())
	if len(out) > maxChars {
		// Back off to a rune boundary so truncation never emits a half-rune.
		cut := maxChars
		for cut > 0 && !utf8.RuneStart(out[cut]) {
			cut--
		}
		out = out[:cut] + "\n…[transcript truncated]…"
	}
	return out
}

// ago renders a duration as a compact relative age for the list rows.
func ago(d time.Duration) string {
	switch {
	case d < time.Minute:
		return "just now"
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	default:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	}
}
