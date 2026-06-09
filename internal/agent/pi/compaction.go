package pi

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Compaction ported from pi's coding-agent (compaction.ts, utils.ts). The policy
// triggers when the context exceeds the model's window minus a reserve, and
// folds the oldest whole turns while keeping roughly KeepRecentTokens of recent
// context. Folding whole turns (by TurnID) is tool-pair-safe by construction —
// an assistant tool_call and its result are never split. The summarizer produces
// pi's structured checkpoint and appends read/modified file tracking.
//
// Deviation, documented: pi's iterative UPDATE-merge (feeding the prior summary
// into the next summarization) is achieved structurally instead. The engine
// folds a span with core.ProjectConversation, so a re-compaction sees the
// earlier span's summary in place of its raw events: each summarization's input
// is bounded by prior-summary + newly folded turns, never total session
// history, without threading the previous summary through the Summarizer port.

const (
	defaultReserveTokens    = 16384
	defaultKeepRecentTokens = 20000
)

// CompactionPolicy is the pi-faithful ports.ContextPolicy.
type CompactionPolicy struct {
	ContextWindow    int // from the model registry
	ReserveTokens    int // 0 = default 16384
	KeepRecentTokens int // 0 = default 20000
}

var _ ports.ContextPolicy = CompactionPolicy{}

func (p CompactionPolicy) reserve() int {
	if p.ReserveTokens > 0 {
		return p.ReserveTokens
	}
	return defaultReserveTokens
}

func (p CompactionPolicy) keepRecent() int {
	if p.KeepRecentTokens > 0 {
		return p.KeepRecentTokens
	}
	return defaultKeepRecentTokens
}

// ShouldCompress triggers when the conversation exceeds the context window minus
// the reserve, matching pi's shouldCompact.
func (p CompactionPolicy) ShouldCompress(totalTokens int, _ []core.Message) bool {
	if p.ContextWindow <= 0 {
		return false
	}
	return totalTokens > p.ContextWindow-p.reserve()
}

// CompressibleRange folds the oldest whole turns, keeping the most recent turns
// whose cumulative estimated tokens reach KeepRecentTokens (always at least the
// newest turn). Turns are TurnID groups in chronological order.
func (p CompactionPolicy) CompressibleRange(events []core.Event) (lo, hi int64, ok bool) {
	turns := core.DistinctTurns(events)
	if len(turns) <= 1 {
		return 0, 0, false
	}
	// Keep decision (pi-specific): keep the most recent turns whose cumulative
	// estimated tokens reach KeepRecentTokens, always at least the newest turn.
	tokens := map[int64]int{}
	for i := range events {
		if m, ok := core.ProjectEvent(events[i]); ok {
			tokens[events[i].TurnID] += core.EstimateTokens(m)
		}
	}
	keep := 0
	acc := 0
	for i := len(turns) - 1; i >= 0; i-- {
		acc += tokens[turns[i]]
		keep++
		if acc >= p.keepRecent() {
			break
		}
	}
	if keep >= len(turns) {
		return 0, 0, false // everything recent enough; nothing old to fold
	}
	fold := map[int64]bool{}
	for _, t := range turns[:len(turns)-keep] {
		fold[t] = true
	}
	return core.SeqSpan(events, fold)
}

// SUMMARIZATION_SYSTEM_PROMPT and the checkpoint template are reproduced verbatim
// from pi.
const summarizationSystemPrompt = `You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.`

const summarizationPrompt = `The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

Keep each section concise. Preserve exact file paths, function names, and error messages. Record only what is needed to continue this task; do not restate general project knowledge that can be rediscovered from the codebase (structure, conventions, build commands).`

// Summarizer is the ports.Summarizer for pi sessions: it asks the model for a
// structured task-state checkpoint of the folded span and appends file
// tracking. It deliberately carries no reasoning level — checkpointing is
// template extraction, not problem solving — so it stays fast and cheap on
// whatever distill model it is given.
type Summarizer struct {
	Provider      ports.LLMProvider
	Model         string
	ReserveTokens int
}

var _ ports.Summarizer = Summarizer{}

func (s Summarizer) Summarize(ctx context.Context, msgs []core.Message) (string, error) {
	convo := serializeConversation(msgs)
	prompt := "<conversation>\n" + convo + "\n</conversation>\n\n" + summarizationPrompt
	reserve := s.ReserveTokens
	if reserve <= 0 {
		reserve = defaultReserveTokens
	}
	req := core.LLMRequest{
		Model:     s.Model,
		Stream:    true,
		MaxTokens: reserve * 4 / 5,
		Messages: []core.Message{
			{Role: core.RoleSystem, Content: summarizationSystemPrompt},
			{Role: core.RoleUser, Content: prompt},
		},
	}
	chunks, err := s.Provider.Stream(ctx, req)
	if err != nil {
		return "", err
	}
	var b strings.Builder
	for ch := range chunks {
		if ch.Err != nil {
			return "", ch.Err
		}
		b.WriteString(ch.ContentDelta)
	}
	summary := b.String()
	if summary == "" {
		return "", fmt.Errorf("summarizer produced no content")
	}
	return summary + formatFileOperations(msgs), nil
}

// serializeConversation renders folded messages to text so the model summarizes
// rather than continues them, matching pi's serializeConversation (tool results
// truncated to 2000 chars).
func serializeConversation(msgs []core.Message) string {
	const toolResultMaxChars = 2000
	var parts []string
	for _, m := range msgs {
		switch m.Role {
		case core.RoleUser:
			if m.Content != "" {
				parts = append(parts, "[User]: "+m.Content)
			}
		case core.RoleAssistant:
			if m.Reasoning != "" {
				parts = append(parts, "[Assistant thinking]: "+m.Reasoning)
			}
			if m.Content != "" {
				parts = append(parts, "[Assistant]: "+m.Content)
			}
			if len(m.ToolCalls) > 0 {
				var calls []string
				for _, tc := range m.ToolCalls {
					calls = append(calls, fmt.Sprintf("%s(%s)", tc.Name, string(tc.Args)))
				}
				parts = append(parts, "[Assistant tool calls]: "+strings.Join(calls, "; "))
			}
		case core.RoleTool:
			if m.Content != "" {
				parts = append(parts, "[Tool result]: "+truncateForSummary(m.Content, toolResultMaxChars))
			}
		case core.RoleSystem:
			// Within a folded span the only system messages are earlier
			// compaction summaries, rendered by ProjectConversation in place of
			// their raw events. Label them so the model merges rather than
			// re-narrates.
			parts = append(parts, "[Earlier summary]: "+m.Content)
		default:
		}
	}
	return strings.Join(parts, "\n\n")
}

func truncateForSummary(text string, maxChars int) string {
	if len(text) <= maxChars {
		return text
	}
	return text[:maxChars] + fmt.Sprintf("\n\n[... %d more characters truncated]", len(text)-maxChars)
}

// formatFileOperations extracts read/modified files from the folded assistant
// tool calls and renders pi's <read-files>/<modified-files> tags.
func formatFileOperations(msgs []core.Message) string {
	read, written, edited := map[string]bool{}, map[string]bool{}, map[string]bool{}
	for _, m := range msgs {
		if m.Role != core.RoleAssistant {
			continue
		}
		for _, tc := range m.ToolCalls {
			var args struct {
				Path string `json:"path"`
			}
			if json.Unmarshal(tc.Args, &args) != nil || args.Path == "" {
				continue
			}
			switch tc.Name {
			case "read":
				read[args.Path] = true
			case "write":
				written[args.Path] = true
			case "edit":
				edited[args.Path] = true
			}
		}
	}
	modified := map[string]bool{}
	for f := range written {
		modified[f] = true
	}
	for f := range edited {
		modified[f] = true
	}
	var readOnly, mod []string
	for f := range read {
		if !modified[f] {
			readOnly = append(readOnly, f)
		}
	}
	for f := range modified {
		mod = append(mod, f)
	}
	sort.Strings(readOnly)
	sort.Strings(mod)

	var sections []string
	if len(readOnly) > 0 {
		sections = append(sections, "<read-files>\n"+strings.Join(readOnly, "\n")+"\n</read-files>")
	}
	if len(mod) > 0 {
		sections = append(sections, "<modified-files>\n"+strings.Join(mod, "\n")+"\n</modified-files>")
	}
	if len(sections) == 0 {
		return ""
	}
	return "\n\n" + strings.Join(sections, "\n\n")
}
