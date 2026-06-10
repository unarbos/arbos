package codingspec

import (
	"context"
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// ChangesArgs are the arguments to changes.
type ChangesArgs struct {
	Path string `json:"path,omitempty" desc:"Optional file or directory whose owning git repo should be inspected. Defaults to the current working directory's repo."`
}

// changesSpec shows the workspace's current git state and, when this session has
// an automatic restore point, the diff from that restore point to the live tree.
// It is the merge surface for local-Cursor-style parallel agents: when a stale
// edit/write fails, the model can inspect what moved, re-read the affected file,
// and retry against the current content.
func changesSpec(cp *checkpointer) tool.Spec {
	spec := tool.NewSpec("changes",
		"Show git status and, when available, changes since this session's latest automatic restore point. Pass path to inspect the git repo that owns that file or directory; omit it for the current working directory's repo. Use after a stale edit/write failure to see what changed before re-reading and retrying.",
		true,
		func(ctx context.Context, a ChangesArgs) (string, error) {
			return cp.changes(ctx, a.Path)
		})
	// Snapshotting a worktree for diff should not race mutators in the same
	// tool batch; serialize it with writes/bash/delegates rather than treating it
	// as a free read-only call.
	return tool.WithAccess(spec, func(ChangesArgs) core.AccessSet {
		return core.AccessSet{Unknown: true}
	})
}

func (c *checkpointer) changes(ctx context.Context, path string) (string, error) {
	repo, err := c.repoForArg(ctx, path)
	if err != nil {
		return "", err
	}

	var b strings.Builder
	status, _ := c.git(ctx, repo, nil, "status", "--short")
	status = strings.TrimSpace(status)
	if status == "" {
		fmt.Fprintf(&b, "Git repo: %s\nGit status: clean\n", repo)
	} else {
		fmt.Fprintf(&b, "Git repo: %s\nGit status:\n", repo)
		b.WriteString(status)
		b.WriteString("\n")
	}

	ref := c.sessionRef(ctx)
	if _, err := c.git(ctx, repo, nil, "rev-parse", "--verify", "-q", ref+"^{commit}"); err != nil {
		b.WriteString("\nNo automatic restore point recorded for this session yet.")
		return b.String(), nil
	}
	now, ok := c.worktreeTree(ctx, repo)
	if !ok {
		b.WriteString("\nCould not snapshot the current worktree to diff against the restore point.")
		return b.String(), nil
	}

	names, _ := c.git(ctx, repo, nil, "diff", "--name-status", ref, now, "--", ".")
	names = strings.TrimSpace(names)
	if names == "" {
		b.WriteString("\nChanges since this session's restore point: none")
		return b.String(), nil
	}
	stat, _ := c.git(ctx, repo, nil, "diff", "--stat", ref, now, "--", ".")
	b.WriteString("\nChanges since this session's restore point:\n")
	if stat = strings.TrimSpace(stat); stat != "" {
		b.WriteString(stat)
		b.WriteString("\n")
	}
	b.WriteString(names)
	return b.String(), nil
}
