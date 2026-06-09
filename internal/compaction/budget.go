// Package compaction provides the reference ports.ContextPolicy: a token-budget
// policy that folds the oldest whole turns once the projected conversation
// exceeds a threshold, keeping the most recent turns intact. It is the one
// place the "when and how much to compress" decision lives — the seam that in
// the Python original was scattered across four call sites (ADR-0014).
package compaction

import (
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// TokenBudget folds the oldest turns when an estimated token count exceeds
// MaxTokens, always leaving the last KeepTurns turns uncompressed.
type TokenBudget struct {
	MaxTokens int // compress when the estimate exceeds this
	KeepTurns int // most recent turns to never compress (min 1)
}

var _ ports.ContextPolicy = TokenBudget{}

func (b TokenBudget) ShouldCompress(totalTokens int, _ []core.Message) bool {
	return b.MaxTokens > 0 && totalTokens > b.MaxTokens
}

// CompressibleRange folds whole turns to avoid splitting an assistant tool_call
// from its results. Turns are identified by TurnID (the engine's per-prompt
// group key); events are stored in order with non-decreasing TurnID, so the
// turns to compress are a contiguous prefix. Already-compressed prefixes are
// handled by re-projection (Project subsumes nested spans), so the policy stays
// a pure span calculation.
func (b TokenBudget) CompressibleRange(events []core.Event) (lo, hi int64, ok bool) {
	keep := b.KeepTurns
	if keep < 1 {
		keep = 1
	}
	turns := core.DistinctTurns(events)
	if len(turns) <= keep {
		return 0, 0, false // not enough history to compress yet
	}
	// Keep decision: fold every turn before the last `keep`.
	fold := map[int64]bool{}
	for _, t := range turns[:len(turns)-keep] {
		fold[t] = true
	}
	return core.SeqSpan(events, fold)
}
