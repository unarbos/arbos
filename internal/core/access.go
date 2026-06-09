package core

import (
	"os"
	"strings"
)

// AccessSet describes the resources one tool call reads and writes, so the
// engine can schedule a batch for maximum safe parallelism: two calls may run
// concurrently only when neither writes a resource the other reads or writes.
//
// Resources are hierarchical, slash-delimited keys — the tool layer uses
// absolute file paths — and a key covers its whole subtree: a read of a
// directory conflicts with a write to any file beneath it. That subtree rule
// is load-bearing because scanning tools (grep, find, ls) read directories
// while mutating tools write individual files; equality-only keys could never
// order those against each other. The zero value (no reads, no writes, not
// unknown) is the natural shape for a tool that touches no workspace resource:
// it conflicts with nothing, so such a batch runs fully parallel.
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

// overlaps reports whether any key in x covers or is covered by a key in y:
// equal keys, or one a path-ancestor of the other. Batches are small (a
// handful of calls, each touching a path or two), so a linear scan is the
// right amount of machinery.
func overlaps(x, y []string) bool {
	for _, a := range x {
		for _, b := range y {
			if a == b || covers(a, b) || covers(b, a) {
				return true
			}
		}
	}
	return false
}

// covers reports whether dir is a path-ancestor of p. Keys are absolute paths
// produced by the tool layer's workspace resolver (filepath), so the ancestor
// test uses the platform separator.
func covers(dir, p string) bool {
	return strings.HasPrefix(p, dir+string(os.PathSeparator))
}
