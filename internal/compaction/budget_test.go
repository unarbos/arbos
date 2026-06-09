package compaction_test

import (
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/compaction"
	"github.com/unarbos/arbos/internal/core"
)

func turnEvent(turn, seq int64) core.Event {
	e := core.NewMessageEvent("s", core.Message{Role: core.RoleUser, Content: "x"}, time.Unix(0, 0))
	e.TurnID = turn
	e.Seq = seq
	return *e
}

func TestShouldCompress(t *testing.T) {
	b := compaction.TokenBudget{MaxTokens: 100, KeepTurns: 2}
	if b.ShouldCompress(50, nil) {
		t.Fatal("under budget should not compress")
	}
	if !b.ShouldCompress(150, nil) {
		t.Fatal("over budget should compress")
	}
	if (compaction.TokenBudget{MaxTokens: 0}).ShouldCompress(1_000_000, nil) {
		t.Fatal("zero budget disables compression")
	}
}

func TestCompressibleRangeKeepsRecentTurns(t *testing.T) {
	// 4 turns, 1 event each. KeepTurns=2 → compress turns 1,2 (seq 0,1).
	events := []core.Event{
		turnEvent(1, 0), turnEvent(2, 1), turnEvent(3, 2), turnEvent(4, 3),
	}
	b := compaction.TokenBudget{MaxTokens: 1, KeepTurns: 2}
	lo, hi, ok := b.CompressibleRange(events)
	if !ok || lo != 0 || hi != 1 {
		t.Fatalf("want [0,1] ok, got [%d,%d] ok=%v", lo, hi, ok)
	}
}

func TestCompressibleRangeNotEnoughHistory(t *testing.T) {
	events := []core.Event{turnEvent(1, 0), turnEvent(2, 1)}
	b := compaction.TokenBudget{MaxTokens: 1, KeepTurns: 2}
	if _, _, ok := b.CompressibleRange(events); ok {
		t.Fatal("with only KeepTurns turns there is nothing to compress")
	}
}
