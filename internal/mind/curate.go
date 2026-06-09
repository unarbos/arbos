package mind

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/core"
)

// curateSession folds the events appended since this session's last checkpoint
// into the global atom set, then advances the checkpoint — atomically and
// incrementally. Best-effort throughout.
func (m *Mind) curateSession(ctx context.Context, sid core.SessionID) {
	cp, err := m.store.AtomCheckpoint(ctx, sid)
	if err != nil {
		m.log.Warn("mind: checkpoint read failed", "session", sid, "err", err)
		return
	}
	events, err := m.store.Events(ctx, sid)
	if err != nil {
		m.log.Warn("mind: events read failed", "session", sid, "err", err)
		return
	}
	var delta []core.Event
	for _, e := range events {
		if e.Seq > cp {
			delta = append(delta, e)
		}
	}
	// Align to completed exchanges: only curate through the last assistant message,
	// dropping any trailing events (a next turn's user prompt whose reply hasn't
	// landed yet). This keeps a question and its answer in the same curation pass
	// instead of splitting them when the worker runs mid-next-turn. The leftover
	// trailing events are picked up by the next pass via the checkpoint.
	lastA := -1
	for i := range delta {
		if p, ok := delta[i].Payload.(core.MessagePayload); ok && p.Message.Role == core.RoleAssistant {
			lastA = i
		}
	}
	if lastA < 0 {
		return // no complete exchange yet; wait for the reply
	}
	delta = delta[:lastA+1]
	maxSeq := delta[len(delta)-1].Seq
	// Distill only the most recent tail (bounded), but advance the checkpoint past
	// the whole aligned delta below — so an oversized backlog is summarized once
	// from its most-relevant end and never reprocessed.
	if len(delta) > maxDeltaEvents {
		delta = delta[len(delta)-maxDeltaEvents:]
	}

	existing := m.mergeCandidates(ctx, delta)
	patch, err := m.distill(ctx, delta, existing)
	if err != nil {
		// Advance the checkpoint anyway: a delta we cannot distill (e.g. too large
		// for the model, transient provider error) must not be retried every turn
		// forever. We lose this slice of memory rather than wedge curation.
		m.log.Warn("mind: distill failed; skipping delta", "session", sid, "to_seq", maxSeq, "err", err)
		_ = m.store.CommitCuration(ctx, sid, nil, nil, maxSeq)
		return
	}
	// Advance the checkpoint even on an empty patch: the delta was considered and
	// held nothing worth remembering, so reprocessing it would only waste calls.
	if err := m.store.CommitCuration(ctx, sid, patch.Set, patch.Forget, maxSeq); err != nil {
		m.log.Warn("mind: commit curation failed", "session", sid, "err", err)
		return
	}
	if len(patch.Set) > 0 || len(patch.Forget) > 0 {
		m.log.Debug("mind: curated", "session", sid, "set", len(patch.Set), "forget", len(patch.Forget), "to_seq", maxSeq)
	}
}

// mergeCandidates picks the existing atoms shown to the curator so it can merge
// (reuse ids) rather than duplicate. It prioritizes atoms FTS-relevant to the
// delta — so a merge target is found even when the set has grown past
// existingCap and the relevant atom is no longer among the most recent — then
// fills the remainder with recent atoms. (Total atom growth is still unbounded;
// eviction/GC is a deliberate non-goal for now.)
func (m *Mind) mergeCandidates(ctx context.Context, delta []core.Event) []core.Atom {
	relevant, _ := m.store.RecallAtoms(ctx, ftsQuery(deltaText(delta)), existingCap)
	recent, _ := m.store.AllAtoms(ctx, existingCap)
	seen := make(map[string]bool, len(relevant)+len(recent))
	out := make([]core.Atom, 0, existingCap)
	for _, a := range relevant {
		if !seen[a.ID] {
			seen[a.ID] = true
			out = append(out, a)
		}
	}
	for _, a := range recent {
		if len(out) >= existingCap {
			break
		}
		if !seen[a.ID] {
			seen[a.ID] = true
			out = append(out, a)
		}
	}
	return out
}

// deltaText concatenates message prose from the delta into the cue used to find
// relevant existing atoms. Tool results are skipped — too noisy to discriminate.
func deltaText(delta []core.Event) string {
	var b strings.Builder
	for _, e := range delta {
		if p, ok := e.Payload.(core.MessagePayload); ok && p.Message.Content != "" {
			b.WriteString(p.Message.Content)
			b.WriteByte(' ')
		}
	}
	return b.String()
}

// patch is the curator's output: atoms to upsert (reusing an existing id is a
// merge) and ids to forget.
type patch struct {
	Set    []core.Atom
	Forget []string
}

const curateSystemPrompt = `You maintain an AI coding agent's long-term memory: a flat set of "atoms". An atom is a short note the agent writes to its future self so it does not have to relearn things.

You are given the EXISTING atoms and the NEW activity from one finished turn. Return only what should change.

Rules:
- Record durable, reusable knowledge: how the codebase/project is structured, where key things live, build/test/run commands, decisions, conventions, stable user preferences. NOT transient chatter or one-off task state.
- Write the subject INTO the note (file paths, package names, command text) so it can be found later by keyword. Each atom is self-contained.
- Each atom <= 60 words, declarative, present tense. Never paste file contents, code blocks, or logs.
- MERGE, don't duplicate: if new activity refines an existing atom, reuse its exact id in "set" with updated content.
- Use "forget" only for atoms now clearly wrong or obsolete.
- ids are short kebab-case slugs describing the subject, e.g. "build-commands", "engine-turn-loop", "user-prefers-uv".
- It is fine to return empty arrays when the turn taught nothing durable.

Respond with ONLY a JSON object, no prose, no markdown fences:
{"set":[{"id":"...","content":"..."}],"forget":["..."]}`

func (m *Mind) distill(ctx context.Context, delta []core.Event, existing []core.Atom) (patch, error) {
	user := "EXISTING ATOMS:\n" + renderExisting(existing) + "\n\nNEW ACTIVITY:\n" + renderEvents(delta, maxDeltaChars)
	req := core.LLMRequest{
		Model:     m.model,
		Stream:    true,
		MaxTokens: 2048,
		Messages: []core.Message{
			{Role: core.RoleSystem, Content: curateSystemPrompt},
			{Role: core.RoleUser, Content: user},
		},
	}
	chunks, err := m.llm.Stream(ctx, req)
	if err != nil {
		return patch{}, err
	}
	var b strings.Builder
	for ch := range chunks {
		if ch.Err != nil {
			return patch{}, ch.Err
		}
		b.WriteString(ch.ContentDelta)
	}
	return parsePatch(b.String())
}

// parsePatch extracts the JSON object from the model's reply (tolerating
// accidental markdown fences or surrounding prose) and maps it to a patch.
func parsePatch(raw string) (patch, error) {
	s := strings.TrimSpace(raw)
	if i := strings.IndexByte(s, '{'); i >= 0 {
		if j := strings.LastIndexByte(s, '}'); j > i {
			s = s[i : j+1]
		}
	}
	var out struct {
		Set []struct {
			ID      string `json:"id"`
			Content string `json:"content"`
		} `json:"set"`
		Forget []string `json:"forget"`
	}
	if err := json.Unmarshal([]byte(s), &out); err != nil {
		return patch{}, fmt.Errorf("parse curation patch: %w", err)
	}
	var p patch
	for _, a := range out.Set {
		id := strings.TrimSpace(a.ID)
		content := strings.TrimSpace(a.Content)
		if id == "" || content == "" {
			continue
		}
		p.Set = append(p.Set, core.Atom{ID: id, Content: content})
	}
	p.Forget = out.Forget
	return p, nil
}

func renderExisting(atoms []core.Atom) string {
	if len(atoms) == 0 {
		return "(none yet)"
	}
	var b strings.Builder
	for _, a := range atoms {
		fmt.Fprintf(&b, "- %s: %s\n", a.ID, a.Content)
	}
	return strings.TrimRight(b.String(), "\n")
}

// renderEvents serializes the delta into a compact transcript for the curator.
// Tool results are truncated; injected-context/usage/compression events carry no
// new knowledge and are skipped. The result is bounded to budgetChars by
// dropping the oldest lines, so a single curation prompt never blows up even if
// individual events are large.
func renderEvents(delta []core.Event, budgetChars int) string {
	const toolMax = 1500
	var parts []string
	for _, e := range delta {
		switch p := e.Payload.(type) {
		case core.MessagePayload:
			msg := p.Message
			switch msg.Role {
			case core.RoleUser:
				if msg.Content != "" {
					parts = append(parts, "[user] "+msg.Content)
				}
			case core.RoleAssistant:
				if msg.Content != "" {
					parts = append(parts, "[assistant] "+msg.Content)
				}
				for _, tc := range msg.ToolCalls {
					parts = append(parts, fmt.Sprintf("[tool-call] %s(%s)", tc.Name, truncate(string(tc.Args), 400)))
				}
			default:
				// System/tool roles never persist as MessagePayload (Validate
				// rejects them); nothing to fold.
			}
		case core.ToolResultPayload:
			if c := p.Result.Content; c != "" {
				parts = append(parts, "[tool-result] "+truncate(c, toolMax))
			}
		}
	}
	// Keep the most recent lines within budget (drop from the front).
	total := 0
	start := len(parts)
	for i := len(parts) - 1; i >= 0; i-- {
		total += len(parts[i]) + 1
		if total > budgetChars {
			break
		}
		start = i
	}
	return strings.Join(parts[start:], "\n")
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + fmt.Sprintf(" …(+%d chars)", len(s)-max)
}
