package core

// AccessSet describes the resources one tool call reads and writes, so the
// engine can schedule a batch for maximum safe parallelism: two calls may run
// concurrently only when neither writes a resource the other reads or writes.
//
// Resources are opaque keys — the tool layer uses absolute file paths — and the
// kernel never interprets them; it only tests them for equality. The zero value
// (no reads, no writes, not unknown) is the natural shape for a pure read-only
// tool: it conflicts with nothing, so an all-read-only batch runs fully
// parallel, exactly as the coarse ToolSchema.ReadOnly flag did.
//
// Unknown marks a call whose footprint cannot be bounded (an arbitrary shell
// command, say). It conflicts with every other call and therefore runs in
// isolation — the honest, safe default for anything the tool layer cannot
// describe in terms of concrete resources.
type AccessSet struct {
	Reads   []string
	Writes  []string
	Unknown bool
}

// Conflicts reports whether two calls must run sequentially. An unbounded
// footprint conflicts with everything; otherwise a conflict exists iff one call
// writes a resource the other reads or writes. Read/read pairs never conflict,
// which is what lets a read-only batch run fully concurrent.
func (a AccessSet) Conflicts(b AccessSet) bool {
	if a.Unknown || b.Unknown {
		return true
	}
	return overlaps(a.Writes, b.Writes) ||
		overlaps(a.Writes, b.Reads) ||
		overlaps(b.Writes, a.Reads)
}

// overlaps reports whether two resource-key slices share any element. Batches
// are small (a handful of calls, each touching a path or two), so a linear scan
// is the right amount of machinery.
func overlaps(x, y []string) bool {
	for _, a := range x {
		for _, b := range y {
			if a == b {
				return true
			}
		}
	}
	return false
}
