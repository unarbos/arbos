package codingspec

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"sort"
)

// errWalkStop aborts a walkTree early without error (e.g. a result limit hit).
var errWalkStop = errors.New("walk stopped")

// walkTree is the one gitignore-aware traversal behind find and grep's native
// fallback: a deterministic sorted DFS under root calling fn for every
// non-ignored entry with its absolute path, slash-separated root-relative
// path, and directory flag. .git directories are always skipped, symlinks are
// visited but never followed, unreadable directories are skipped, and
// .gitignore files above root (up to the enclosing repo root) are honored.
// fn may return errWalkStop to end the walk cleanly.
func walkTree(ctx context.Context, root string, fn func(abs, rel string, isDir bool) error) error {
	m := &ignoreMatcher{}
	loadAncestorIgnores(m, root)

	var walk func(dir, rel string) error
	walk = func(dir, rel string) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		if m.push(dir) {
			defer m.pop()
		}
		ents, err := os.ReadDir(dir)
		if err != nil {
			return nil // unreadable directories are skipped, not fatal
		}
		sort.Slice(ents, func(i, j int) bool { return ents[i].Name() < ents[j].Name() })
		for _, e := range ents {
			name := e.Name()
			isDir := e.IsDir()
			if name == ".git" && isDir {
				continue
			}
			abs := filepath.Join(dir, name)
			childRel := name
			if rel != "" {
				childRel = rel + "/" + name
			}
			if m.ignored(abs, isDir) {
				continue
			}
			if err := fn(abs, childRel, isDir); err != nil {
				return err
			}
			if isDir {
				if err := walk(abs, childRel); err != nil {
					return err
				}
			}
		}
		return nil
	}
	err := walk(root, "")
	if errors.Is(err, errWalkStop) {
		return nil
	}
	return err
}
