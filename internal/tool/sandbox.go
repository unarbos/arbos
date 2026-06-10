package tool

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// Resolve turns a user-supplied path into an absolute, cleaned path: relative
// paths join to base (the workspace cwd), absolute paths are taken as given. It
// does NOT confine. arbos trusts its agents and assumes a human is watching, so
// a path that points outside base — via "../", an absolute path, or a symlink —
// resolves like any other. This is the default every file tool shares, and it
// is what lets an agent edit up the tree, a sibling repo, or anywhere the user
// running arbos can. An empty base means the current directory.
//
// To run an agent inside a jail instead — untrusted or unattended work that must
// stay in one tree — a host uses ResolveWithin in place of Resolve. The boundary
// is opt-in, not the default.
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
	// Natural join: "../" traverses above base like a shell, so an agent can
	// reach a sibling repo or a parent dir. The boundary, if wanted, is
	// ResolveWithin's job — not a silent clamp here.
	return filepath.Join(absBase, rel), nil
}

// ResolveWithin is Resolve plus a workspace boundary: it refuses any path that
// escapes root, whether lexically ("../") or through a symlink inside root that
// points back out. It is the opt-in sandbox — wire it where Resolve would go to
// confine a host to a single tree (e.g. running an untrusted backend, or an
// unattended fleet where a stray write must not reach $HOME or /etc). The
// default host does not call it; see Resolve for the rationale.
func ResolveWithin(root, rel string) (string, error) {
	abs, err := Resolve(root, rel)
	if err != nil {
		return "", err
	}
	absRoot, err := filepath.Abs(root)
	if err != nil {
		return "", err
	}
	if !withinRoot(abs, absRoot) {
		return "", fmt.Errorf("path %q is outside the workspace root; use a path relative to the root", rel)
	}

	// Symlink guard: resolve symlinks on the longest existing prefix of the
	// target (the file itself may not exist yet, e.g. write) and re-check against
	// the real root. This catches a symlink ancestor inside the workspace that
	// points outside it. Resolving the root too handles the case where the
	// workspace path is itself reached through a symlink (common for temp dirs).
	realRoot, err := filepath.EvalSymlinks(absRoot)
	if err != nil {
		realRoot = absRoot
	}
	if real, ok := evalExistingPrefix(abs); ok && !withinRoot(real, realRoot) {
		return "", fmt.Errorf("path %q escapes the workspace root via a symlink", rel)
	}
	return abs, nil
}

// withinRoot reports whether p is root itself or lies beneath it.
func withinRoot(p, root string) bool {
	return p == root || strings.HasPrefix(p, root+string(os.PathSeparator))
}

// evalExistingPrefix resolves symlinks on the longest existing ancestor of p and
// re-appends the non-existent tail, so a not-yet-created file still gets a
// symlink-resolved parent. ok is false if no ancestor exists.
func evalExistingPrefix(p string) (string, bool) {
	cur := p
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
