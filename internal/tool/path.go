package tool

import (
	"path/filepath"
	"slices"
)

// Resolve turns a user-supplied path into an absolute, cleaned path: relative
// paths join to base (the session's working directory), absolute paths are
// taken as given, and "../" traverses up like a shell. The working directory is
// where an agent starts, not a wall. arbos agents reach anything the user
// running arbos can, because a human is watching and git is the net. There is
// deliberately no confinement here: if a host ever truly needs a jailed agent
// (an untrusted backend, an unattended fleet), that is OS-level isolation, such as a
// container or a dedicated user — not a path-string check. An empty base means
// the current directory.
func Resolve(base, rel string) (string, error) {
	if base == "" {
		base = "."
	}
	absBase, err := filepath.Abs(base)
	if err != nil {
		return "", err
	}
	if filepath.IsAbs(rel) {
		return filepath.Clean(rel), nil
	}
	return filepath.Join(absBase, rel), nil
}

// ResourceKey returns the canonical key used for scheduling and mutation
// coordination. Resolve stays shell-like for IO, but concurrency needs a stable
// identity for "the same file" across symlinks and alternate spellings. It
// resolves symlinks on the longest existing prefix, then appends any missing
// tail so not-yet-created files under a symlinked directory still share the
// same key.
func ResourceKey(path string) string {
	if real, ok := evalExistingPrefix(path); ok {
		return real
	}
	return filepath.Clean(path)
}

func evalExistingPrefix(path string) (string, bool) {
	cur := filepath.Clean(path)
	var tail []string
	for {
		if real, err := filepath.EvalSymlinks(cur); err == nil {
			slices.Reverse(tail)
			parts := append([]string{real}, tail...)
			return filepath.Join(parts...), true
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			return "", false
		}
		tail = append(tail, filepath.Base(cur))
		cur = parent
	}
}
