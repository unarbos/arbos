package tool

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
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
//
// The one exception is the agent's own key material (ADR-0035/ADR-0041 D3): a
// prompt-injected agent must not be able to read or overwrite the device key
// (now also the homeserver identity root) or the per-directory homeserver
// keys/state. Resolve denies those specific paths — this is a narrow secret
// denylist, not confinement: everything else, including .arbos/prompts and
// .arbos/skills, stays reachable.
func Resolve(base, rel string) (string, error) {
	if base == "" {
		base = "."
	}
	absBase, err := filepath.Abs(base)
	if err != nil {
		return "", err
	}
	resolved := filepath.Clean(rel)
	if !filepath.IsAbs(rel) {
		resolved = filepath.Join(absBase, rel)
	}
	if deniedPath(resolved) {
		return "", fmt.Errorf("%q is arbos key/identity material and is not accessible to tools", rel)
	}
	return resolved, nil
}

// deniedPath reports whether an absolute path is arbos secret/identity material
// the agent's tools must not touch (ADR-0035): the machine-level config home
// (the device key + managed-secret vault), and the per-directory homeserver
// keys/state (.arbos/matrix, .arbos/identity). The rest of .arbos is fair game.
func deniedPath(abs string) bool {
	sep := string(os.PathSeparator)
	if cfg, err := os.UserConfigDir(); err == nil {
		root := filepath.Join(cfg, "arbos")
		if abs == root || strings.HasPrefix(abs, root+sep) {
			return true
		}
	}
	// Match the named dir and anything beneath it, anywhere in the tree.
	withTrailer := abs + sep
	for _, secret := range []string{".arbos" + sep + "matrix", ".arbos" + sep + "identity"} {
		if strings.Contains(withTrailer, sep+secret+sep) {
			return true
		}
	}
	return false
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
