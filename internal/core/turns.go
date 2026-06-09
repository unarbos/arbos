package core

// DistinctTurns returns the distinct TurnIDs in first-seen (chronological)
// order. Events are stored in order with non-decreasing TurnID, so this is the
// turn timeline both compaction policies fold over. It is the shared half of the
// "fold whole turns" machinery; each policy supplies only its own keep decision.
func DistinctTurns(events []Event) []int64 {
	var turns []int64
	seen := map[int64]bool{}
	for i := range events {
		t := events[i].TurnID
		if !seen[t] {
			seen[t] = true
			turns = append(turns, t)
		}
	}
	return turns
}

// SeqSpan returns the inclusive [lo,hi] event-Seq span covering every event
// whose TurnID is in fold. ok is false when no event matches (nothing to fold).
// Folding whole turns (not arbitrary spans) is what keeps an assistant tool_call
// from being split from its results.
func SeqSpan(events []Event, fold map[int64]bool) (lo, hi int64, ok bool) {
	first := true
	for i := range events {
		if !fold[events[i].TurnID] {
			continue
		}
		if first {
			lo = events[i].Seq
			first = false
		}
		hi = events[i].Seq
	}
	return lo, hi, !first
}
