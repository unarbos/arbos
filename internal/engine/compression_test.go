package engine_test

import (
	"context"
	"strings"
	"testing"

	"github.com/unarbos/arbos/internal/compaction"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/ports"
)

// stubSummarizer returns a fixed marker so the test is deterministic.
type stubSummarizer struct{}

func (stubSummarizer) Summarize(ctx context.Context, msgs []core.Message) (string, error) {
	return "SUMMARY", nil
}

// chattyProvider always answers with a long string and no tool calls, so each
// turn grows the conversation until the budget triggers compression.
type chattyProvider struct{}

func (chattyProvider) Name() string                     { return "chatty" }
func (chattyProvider) Capabilities() ports.Capabilities { return ports.Capabilities{} }
func (chattyProvider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 2)
	go func() {
		defer close(out)
		out <- core.LLMChunk{ContentDelta: strings.Repeat("token ", 200)}
		out <- core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 10}}
	}()
	return out, nil
}

func TestCompressionFoldsOldTurns(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	store := fake.NewStore()
	eng := engine.New(
		chattyProvider{},
		fake.Tools{},
		store,
		fake.NewClock(),
		engine.Config{Model: "chatty", MaxIterations: 5},
		engine.WithContextPolicy(compaction.TokenBudget{MaxTokens: 200, KeepTurns: 2}, stubSummarizer{}),
	)
	conv, err := eng.StartSession(ctx, "s-compress")
	if err != nil {
		t.Fatal(err)
	}

	// Drive several turns; each adds a long assistant message, eventually
	// crossing the budget so the next turn compresses the oldest turns.
	for i := 0; i < 5; i++ {
		conv.Send(core.PromptIntent{Text: "tell me more"})
		drain(t, conv)
	}

	events, err := store.Events(ctx, "s-compress")
	if err != nil {
		t.Fatal(err)
	}
	var compressions int
	for _, ev := range events {
		if cp, ok := ev.Payload.(core.CompressionPayload); ok {
			compressions++
			if cp.Summary != "SUMMARY" {
				t.Fatalf("summarizer output not used: %q", cp.Summary)
			}
		}
	}
	if compressions == 0 {
		t.Fatal("expected at least one compression event after exceeding the budget")
	}

	// The projection must contain the summary and stay valid (folded spans gone).
	msgs := core.Project(events, "sys")
	foundSummary := false
	for _, m := range msgs {
		if strings.Contains(m.Content, "SUMMARY") {
			foundSummary = true
		}
	}
	if !foundSummary {
		t.Fatal("compressed summary not rendered in the projection")
	}
}
