package codingspec

import (
	"context"
	"hash/fnv"
	"strconv"
	"sync"

	"github.com/unarbos/arbos/internal/obs"
)

// readLedger remembers, per file, which line ranges read has already shown the
// model this turn and the file version they were shown at. read consults it to
// skip re-emitting content the model has already seen — the scarce resource is
// the context window, not the disk. One ledger is created per coding toolset
// (see Specs), shared across that session's turns; its memory is scoped to the
// current turn (see reconcile) because compaction can fold an earlier turn's
// read out of the model's context, so "already shown" only holds within a turn.
//
// Concurrency: read is read-only and two reads of the same file do not conflict,
// so the scheduler can dispatch them in the same wave (engine.dispatchScheduled).
// The mutex makes the shared map safe for that — the "isolate shared state
// between parallel actors" corollary.
type readLedger struct {
	mu    sync.Mutex
	turn  int64
	files map[string]fileMemory
}

// fileMemory is what the ledger knows about one absolute path: the file version
// its ranges were captured at, and the merged set of shown line ranges. Ranges
// are 1-based inclusive, sorted ascending, and non-overlapping (mergeRange keeps
// them so). When the version changes the ranges are dropped — a line number from
// an old version no longer denotes the same line.
type fileMemory struct {
	version string
	shown   []lineRange
}

type lineRange struct{ start, end int }

// ledgerVerdict is reconcile's answer for one read. redundant means the shown
// span was already fully shown at this version (so the caller can skip the
// content and return a pointer). fileChanged means we had ranges for this path
// at a different version — the content is shown fresh, with a note.
type ledgerVerdict struct {
	redundant   bool
	fileChanged bool
}

func newReadLedger() *readLedger {
	return &readLedger{files: map[string]fileMemory{}}
}

// reconcile records that [start,end] of path (at version) is about to be shown
// in the given turn, and reports how it relates to what was shown before. A
// non-positive or empty span (nothing actually shown) is a no-op. A nil ledger
// disables dedup entirely (zero verdict), so a toolset assembled without one —
// or a future caller — is unaffected.
//
// A newer turn wipes all memory first: turn ids are monotonic and turns run one
// at a time per session, so a higher turn means the previous turn finished and
// its reads may have been compacted away. turn == 0 (no correlation in ctx, e.g.
// a test) never advances, degrading gracefully to whole-session memory.
//
// Within-turn dedup is only safe because compaction never folds the active turn
// (TokenBudget.CompressibleRange keeps it), so a read skipped here is still in
// the model's context. That invariant lives in the compaction policy, not here;
// if it ever changes, this turn-scoping must tighten with it.
func (l *readLedger) reconcile(turn int64, path, version string, start, end int) ledgerVerdict {
	if l == nil || start <= 0 || end < start {
		return ledgerVerdict{}
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if turn > l.turn {
		l.turn = turn
		l.files = map[string]fileMemory{}
	}
	fm, ok := l.files[path]
	switch {
	case !ok:
		l.files[path] = fileMemory{version: version, shown: []lineRange{{start, end}}}
		return ledgerVerdict{}
	case fm.version != version:
		l.files[path] = fileMemory{version: version, shown: []lineRange{{start, end}}}
		return ledgerVerdict{fileChanged: true}
	default:
		v := ledgerVerdict{redundant: covers(fm.shown, start, end)}
		fm.shown = mergeRange(fm.shown, lineRange{start, end})
		l.files[path] = fm
		return v
	}
}

// peekVersion reports the version of path the model last saw this turn —
// via read or a mutator's noteVersion — without recording anything. edit uses
// it as a staleness guard: a disk version that differs from what the model saw
// means another actor (a background job, an external process) wrote the file.
// The turn-wipe rule matches reconcile; a nil ledger or unseen path reports
// false, so the guard degrades to "no guard" rather than blocking edits.
func (l *readLedger) peekVersion(turn int64, path string) (string, bool) {
	if l == nil {
		return "", false
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if turn > l.turn {
		l.turn = turn
		l.files = map[string]fileMemory{}
		return "", false
	}
	fm, ok := l.files[path]
	if !ok {
		return "", false
	}
	return fm.version, true
}

// noteVersion records that path now holds version — a mutator (write, edit)
// just produced it — without marking any lines as shown. A version change
// drops the shown ranges: their line numbers no longer denote the same lines.
// This keeps peekVersion's staleness guard from misfiring on the model's own
// writes.
func (l *readLedger) noteVersion(turn int64, path, version string) {
	if l == nil {
		return
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if turn > l.turn {
		l.turn = turn
		l.files = map[string]fileMemory{}
	}
	fm := l.files[path]
	if fm.version != version {
		fm.shown = nil
	}
	fm.version = version
	l.files[path] = fm
}

// covers reports whether [s,e] is fully contained in the union of ranges, which
// are sorted ascending and non-overlapping. It walks once, advancing the
// still-needed line past each range that reaches it; a gap before that line
// means the span is not fully covered.
func covers(ranges []lineRange, s, e int) bool {
	need := s
	for _, r := range ranges {
		if r.start > need {
			return false
		}
		if r.end >= need {
			need = r.end + 1
			if need > e {
				return true
			}
		}
	}
	return need > e
}

// mergeRange inserts nr into a sorted, non-overlapping range list, folding any
// ranges it overlaps or abuts (adjacency counts, so [1,5]+[6,9] becomes [1,9]).
// The input order is preserved, so the result stays sorted and non-overlapping.
func mergeRange(ranges []lineRange, nr lineRange) []lineRange {
	merged := make([]lineRange, 0, len(ranges)+1)
	inserted := false
	for _, r := range ranges {
		switch {
		case r.end+1 < nr.start:
			merged = append(merged, r)
		case nr.end+1 < r.start:
			if !inserted {
				merged = append(merged, nr)
				inserted = true
			}
			merged = append(merged, r)
		default:
			if r.start < nr.start {
				nr.start = r.start
			}
			if r.end > nr.end {
				nr.end = r.end
			}
		}
	}
	if !inserted {
		merged = append(merged, nr)
	}
	return merged
}

// fileVersion is a cheap content fingerprint used to detect that a file changed
// between two reads in a session. fnv64a is not cryptographic — it only needs to
// change when the bytes do, and read already holds the bytes, so this adds one
// linear pass over content read anyway.
func fileVersion(data []byte) string {
	h := fnv.New64a()
	_, _ = h.Write(data)
	return strconv.FormatUint(h.Sum64(), 16)
}

// turnFromContext reads the engine's per-turn id off the dispatch context. The
// kernel already stamps it there (obs.Correlation) for every turn, so the read
// tool scopes its ledger to a turn without any new kernel seam. Absent (0) means
// "no turn boundary known" — the ledger then never expires (see reconcile).
func turnFromContext(ctx context.Context) int64 {
	c, _ := obs.From(ctx)
	return c.TurnID
}
