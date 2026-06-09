package pi_test

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/agent/pi"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/mind"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/sqlite"
)

// This is the overflow proof for the unified memory/compaction system: one
// SQLite-backed session driven past ShouldCompress twice, asserting
//
//  1. compression events persist and the projection stays valid (no orphan
//     tool results — whole-turn folding is tool-pair-safe);
//  2. the checkpoint is the summarizer's output, with exact paths preserved;
//  3. re-compaction input is BOUNDED: the second summarization sees the first
//     span as "[Earlier summary]: …", never as raw history;
//  4. knowledge from the folded span survives as atoms and is re-injected by
//     recall in a later turn — compaction loses nothing here.
//
// The provider is scripted by request shape, so the whole loop — policy,
// summarizer, projection, curation, recall — runs for real, hermetically.

// scriptedProvider routes on the request's system prompt: summarization and
// curation requests get canned distiller replies; everything else is a main
// turn that calls the ls tool once and then emits large unique filler so the
// conversation outgrows the context budget.
type scriptedProvider struct {
	mu             sync.Mutex
	summarizeCalls []string // captured summarizer inputs, in order
}

const checkpointPath = "internal/billing/parse.go"

func (p *scriptedProvider) Name() string                     { return "scripted" }
func (p *scriptedProvider) Capabilities() ports.Capabilities { return ports.Capabilities{Tools: true} }

func (p *scriptedProvider) Stream(_ context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 4)
	defer close(out)

	system := ""
	if len(req.Messages) > 0 && req.Messages[0].Role == core.RoleSystem {
		system = req.Messages[0].Content
	}
	switch {
	case strings.Contains(system, "context summarization assistant"):
		p.mu.Lock()
		p.summarizeCalls = append(p.summarizeCalls, req.Messages[len(req.Messages)-1].Content)
		n := len(p.summarizeCalls)
		p.mu.Unlock()
		out <- core.LLMChunk{ContentDelta: fmt.Sprintf(
			"## Goal\nMigrate the billing parser to v2 in %s (CP-MARK-%d)\n\n## Next Steps\n1. Continue the migration", checkpointPath, n)}
	case strings.Contains(system, "long-term memory"):
		out <- core.LLMChunk{ContentDelta: fmt.Sprintf(
			`{"set":[{"id":"billing-parser-location","content":"The billing parser lives in %s; build it with make billing."}],"forget":[]}`, checkpointPath)}
	default:
		// Main turn: request the ls tool first, then answer with unique filler.
		last := req.Messages[len(req.Messages)-1]
		if last.Role == core.RoleTool {
			turn := 0
			for _, m := range req.Messages {
				if m.Role == core.RoleUser {
					turn++
				}
			}
			out <- core.LLMChunk{ContentDelta: fmt.Sprintf("FILLER-%d %s", turn, strings.Repeat("alpha beta gamma delta ", 100))}
		} else {
			out <- core.LLMChunk{ToolCalls: []core.ToolCall{{
				ID: fmt.Sprintf("call-%d", len(req.Messages)), Name: "ls", Args: json.RawMessage(`{"path":"."}`),
			}}}
		}
	}
	out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 1}}
	return out, nil
}

func (p *scriptedProvider) summaries() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.summarizeCalls...)
}

func drainTurn(t *testing.T, conv *engine.Conversation) {
	t.Helper()
	timeout := time.After(15 * time.Second)
	for {
		select {
		case env, ok := <-conv.Events():
			if !ok {
				t.Fatal("events channel closed mid-turn")
			}
			switch ev := env.Event.(type) {
			case core.TurnComplete:
				return
			case core.ErrorEvent:
				t.Fatalf("turn error: %s: %s", ev.Category, ev.Err)
			}
		case <-timeout:
			t.Fatal("turn did not complete in time")
		}
	}
}

func TestOverflowFoldsCurateAndRecall(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	store, err := sqlite.Open(filepath.Join(t.TempDir(), "sessions.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()

	provider := &scriptedProvider{}
	theMind := mind.New(store, provider, "distill", nil)
	defer theMind.Close()

	// Budget chosen so each ~575-token filler turn crosses the trigger
	// (window-reserve = 1500) every couple of turns, while KeepRecentTokens
	// keeps roughly one full turn — forcing at least two folds over six turns.
	eng := engine.New(provider, fake.Tools{}, store, fake.NewClock(),
		engine.Config{Model: "main", MaxIterations: 6},
		engine.WithContextPolicy(
			pi.CompactionPolicy{ContextWindow: 2500, ReserveTokens: 1000, KeepRecentTokens: 600},
			pi.Summarizer{Provider: provider, Model: "distill"},
		),
		engine.WithContextInjector(theMind.Recall),
		engine.WithTurnEnd(theMind.Curate),
	)

	const sid = core.SessionID("s-overflow")
	conv, err := eng.StartSession(ctx, sid)
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 6; i++ {
		conv.Send(core.PromptIntent{Text: "continue"})
		drainTurn(t, conv)
	}

	// --- 1. Compression fired (twice) and the projection stays valid. ---
	events, err := store.Events(ctx, sid)
	if err != nil {
		t.Fatal(err)
	}
	var folds []core.CompressionPayload
	for _, ev := range events {
		if cp, ok := ev.Payload.(core.CompressionPayload); ok {
			folds = append(folds, cp)
		}
	}
	if len(folds) < 2 {
		t.Fatalf("expected at least 2 compressions, got %d", len(folds))
	}

	// --- 2. The checkpoint is the summarizer's output, paths intact. ---
	for i, cp := range folds {
		if !strings.Contains(cp.Summary, checkpointPath) || !strings.Contains(cp.Summary, "## Goal") {
			t.Fatalf("fold %d summary lost the checkpoint structure or path: %q", i, cp.Summary)
		}
	}

	// Tool-pair integrity: in the folded projection every tool result must
	// still follow an assistant message carrying its call.
	assertToolPairs(t, core.Project(events, "sys"))

	// --- 3. Re-compaction input is bounded. ---
	inputs := provider.summaries()
	if len(inputs) < 2 {
		t.Fatalf("expected at least 2 summarizer calls, got %d", len(inputs))
	}
	second := inputs[1]
	if !strings.Contains(second, "[Earlier summary]: ") || !strings.Contains(second, "CP-MARK-1") {
		t.Fatalf("second summarization must fold the first checkpoint, got:\n%s", second)
	}
	if strings.Contains(second, "FILLER-1 ") {
		t.Fatal("second summarization re-fed raw history from the already-folded span")
	}

	// --- 4. Folded knowledge survives: curation produced the atom… ---
	deadline := time.After(10 * time.Second)
	for {
		atoms, err := store.AllAtoms(ctx, 10)
		if err != nil {
			t.Fatal(err)
		}
		found := false
		for _, a := range atoms {
			if a.ID == "billing-parser-location" {
				found = true
			}
		}
		if found {
			break
		}
		select {
		case <-deadline:
			t.Fatal("curation never produced the billing-parser atom")
		case <-time.After(50 * time.Millisecond):
		}
	}

	// …and recall re-injects it in a later turn.
	conv.Send(core.PromptIntent{Text: "where does the billing parser live?"})
	drainTurn(t, conv)
	events, err = store.Events(ctx, sid)
	if err != nil {
		t.Fatal(err)
	}
	recalled := false
	for _, ev := range events {
		p, ok := ev.Payload.(core.ContextPayload)
		if !ok {
			continue
		}
		for _, seg := range p.Segments {
			if seg.Source == core.SourceMemory && strings.Contains(seg.Content, checkpointPath) {
				recalled = true
			}
		}
	}
	if !recalled {
		t.Fatal("knowledge from the folded span was not recalled into the later turn")
	}
}

// assertToolPairs fails if any projected tool result lacks a preceding
// assistant message carrying the matching tool call — the corruption whole-turn
// folding exists to prevent.
func assertToolPairs(t *testing.T, msgs []core.Message) {
	t.Helper()
	pending := map[string]bool{}
	for i, m := range msgs {
		switch m.Role {
		case core.RoleAssistant:
			for _, tc := range m.ToolCalls {
				pending[tc.ID] = true
			}
		case core.RoleTool:
			if !pending[m.ToolCallID] {
				t.Fatalf("orphan tool result at msg %d (call id %q): folding split a tool pair", i, m.ToolCallID)
			}
			delete(pending, m.ToolCallID)
		case core.RoleSystem, core.RoleUser:
		}
	}
	if len(pending) > 0 {
		t.Fatalf("assistant tool calls left without results after folding: %v", pending)
	}
}
