package tool

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// Resolve joins a user-supplied relative path to root and refuses any path that
// escapes root, so a file tool cannot read or clobber files outside the
// workspace. It guards two escapes: lexical traversal ("../") and symlink
// traversal (a symlink inside root pointing out). An empty root means the
// current directory. This is the single home for the workspace sandbox guard
// that every file tool shares.
func Resolve(root, rel string) (string, error) {
	if root == "" {
		root = "."
	}
	absRoot, err := filepath.Abs(root)
	if err != nil {
		return "", err
	}
	joined := filepath.Join(absRoot, filepath.Clean("/"+rel))
	if !withinRoot(joined, absRoot) {
		return "", fmt.Errorf("path %q escapes the workspace root", rel)
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
	if real, ok := evalExistingPrefix(joined); ok && !withinRoot(real, realRoot) {
		return "", fmt.Errorf("path %q escapes the workspace root via a symlink", rel)
	}
	return joined, nil
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
