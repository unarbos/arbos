package core

import "fmt"

// Upcast migrates a persisted event from an older schema version to the current
// one. The log is append-only and forward-only: a newer binary must be able to
// read rows written by an older one, so every byte-backed store calls Upcast on
// read (ADR-0010). It is the single, tested home for migration rules — stores
// never re-implement them.
//
// Today CurrentEventVersion == 1, so this is the identity. When a payload's
// persisted shape changes, bump CurrentEventVersion and add a version rule to the
// switch; the loop then walks an old row up to current one step at a time. For
// example, a v1->v2 change adds `case 1:` that rewrites ev and sets
// ev.Version = 2, after which the loop re-evaluates until it reaches current.
func Upcast(ev Event) (Event, error) {
	for ev.Version < CurrentEventVersion {
		switch ev.Version {
		default:
			return Event{}, fmt.Errorf("upcast: no rule for event version %d (current %d)", ev.Version, CurrentEventVersion)
		}
	}
	return ev, nil
}
