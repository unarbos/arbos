package codingspec

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/unarbos/arbos/internal/tool"
)

// UndoArgs are the arguments to undo.
type UndoArgs struct {
	Path string `json:"path,omitempty" desc:"Optional file or directory whose owning git repo should be restored. Defaults to the current working directory's repo."`
}

// undoSpec restores the working tree to the restore point taken before the most
// recent turn's first edit. It is the deliberate counterpart to the automatic
// snapshot — the model (or the user, via the model) reaches for it when a turn
// went wrong. Not read-only: it mutates the working tree, so it serializes
// against other tool calls.
func undoSpec(cp *checkpointer) tool.Spec {
	return tool.NewSpec("undo",
		"Restore a git worktree to this session's latest automatic restore point. Pass path to restore the git repo that owns that file or directory; omit it for the current working directory's repo. Modified/deleted files are reverted and files created since that restore point are removed. Use when an edit went wrong or the user asks to undo recent changes. Works only inside a git repository. This is a repo restore, not a surgical per-agent rollback.",
		false,
		func(ctx context.Context, a UndoArgs) (string, error) {
			return cp.restore(ctx, a.Path)
		})
}

// restore reverts the working tree to the session's latest restore point: tracked
// files go back to the snapshot (reverting modifications and recreating
// deletions) and files created since the snapshot are removed. The added set is
// computed before restoring, by diffing the restore point against a fresh tree of
// the current worktree — `git diff <ref>` alone would miss untracked files.
func (c *checkpointer) restore(ctx context.Context, path string) (string, error) {
	repo, err := c.repoForArg(ctx, path)
	if err != nil {
		return "", err
	}
	ref := c.sessionRef(ctx)
	if _, err := c.git(ctx, repo, nil, "rev-parse", "--verify", "-q", ref+"^{commit}"); err != nil {
		return "", fmt.Errorf("nothing to undo: no restore point recorded for %s in this session", repo)
	}

	added := c.addedSince(ctx, repo, ref)
	if out, err := c.git(ctx, repo, nil, "restore", "--source="+ref, "--worktree", "--", "."); err != nil {
		return "", fmt.Errorf("undo failed: %s", strings.TrimSpace(out+" "+err.Error()))
	}

	removed := 0
	for _, name := range added {
		if err := os.Remove(filepath.Join(repo, name)); err == nil {
			removed++
		}
	}

	switch removed {
	case 0:
		return fmt.Sprintf("Restored %s to this session's latest restore point.", repo), nil
	case 1:
		return fmt.Sprintf("Restored %s to this session's latest restore point, and removed 1 file created since then.", repo), nil
	default:
		return fmt.Sprintf("Restored %s to this session's latest restore point, and removed %d files created since then.", repo, removed), nil
	}
}

// addedSince returns the paths present in the working tree but absent from ref —
// the files created since the restore point, which restore leaves behind and undo
// must remove. It stages the current tree into a throwaway index and diffs that
// tree against ref so untracked-but-created files are caught (a plain
// `git diff <ref>` skips them).
func (c *checkpointer) addedSince(ctx context.Context, repo, ref string) []string {
	now, ok := c.worktreeTree(ctx, repo)
	if !ok {
		return nil
	}
	out, err := c.git(ctx, repo, nil, "diff", "--name-only", "--diff-filter=A", ref, now, "--", ".")
	if err != nil {
		return nil
	}
	var files []string
	for _, name := range strings.Split(out, "\n") {
		if name = strings.TrimSpace(name); name != "" {
			files = append(files, name)
		}
	}
	return files
}

func (c *checkpointer) repoForArg(ctx context.Context, path string) (string, error) {
	target := c.root
	if strings.TrimSpace(path) != "" {
		abs, err := tool.Resolve(c.root, path)
		if err != nil {
			return "", err
		}
		target = abs
	}
	if repo, ok := c.repoForTarget(ctx, target); ok {
		return repo, nil
	}
	if path == "" {
		return "", fmt.Errorf("current working directory is not inside a git repository")
	}
	return "", fmt.Errorf("%s is not inside a git repository", path)
}
