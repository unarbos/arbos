package codingspec

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"

	"github.com/unarbos/arbos/internal/obs"
)

// checkpointer snapshots whichever git worktree owns a mutation target before
// the agent's first mutation of that repo in a turn. It is git-native by design:
// the snapshot is a commit object capturing the full worktree (tracked and
// untracked, .gitignore respected), written to a per-session ref whose reflog
// becomes the restore-point history — git is the store, the timeline, and the
// GC. It never touches the user's index, HEAD, stash, or working tree; it only
// adds objects and moves an arbos-owned ref. Outside a git repo it is a no-op, so
// a non-repo path is simply not undo-covered.
//
// One checkpointer per coding toolset (see Specs); the mutating tools (write,
// edit, bash) call ensure before they change anything. Turn-scoped like the read
// ledger: a new turn id arms a fresh snapshot per owning repo, so there is one
// restore point per touched repo per turn. Restoring is the deliberate, opt-in
// half — the undo tool — so the automatic path can only ever protect, never
// destroy. A restore point is a repo/worktree snapshot, not a per-file ownership
// record: undo rolls that shared repo back to that moment, which is the
// local-Cursor model.
type checkpointer struct {
	root  string
	mu    sync.Mutex
	turns map[string]int64 // repo root -> last turn checkpointed; 0 = none yet
}

func newCheckpointer(root string) *checkpointer {
	return &checkpointer{root: root, turns: map[string]int64{}}
}

// gitIdentity lets commit-tree succeed even when the repo has no user.name/email
// configured — the checkpoint must never fail for want of a committer.
var gitIdentity = []string{
	"GIT_AUTHOR_NAME=arbos",
	"GIT_AUTHOR_EMAIL=arbos@localhost",
	"GIT_COMMITTER_NAME=arbos",
	"GIT_COMMITTER_EMAIL=arbos@localhost",
}

// ensure takes the turn's snapshot of the session cwd repo on the first mutation
// of that turn. Bash uses this because arbitrary shell commands do not declare
// file targets.
func (c *checkpointer) ensure(ctx context.Context) {
	repo, ok := c.repoForTarget(ctx, c.root)
	if !ok {
		return
	}
	c.ensureRepo(ctx, repo)
}

// ensurePath snapshots the git worktree that owns abs before a file-targeted
// mutation. It returns false when abs is outside any git worktree, letting the
// caller surface an "not undo-covered" note without blocking the actual edit.
func (c *checkpointer) ensurePath(ctx context.Context, abs string) bool {
	repo, ok := c.repoForTarget(ctx, abs)
	if !ok {
		return false
	}
	return c.ensureRepo(ctx, repo)
}

// ensureRepo records the turn's restore point for repo. Errors are intentionally
// swallowed: a restore point is a safety net, not a gate — a repo mid-rebase or
// in any odd state must never block the agent's actual work.
func (c *checkpointer) ensureRepo(ctx context.Context, repo string) bool {
	turn := turnFromContext(ctx)
	c.mu.Lock()
	defer c.mu.Unlock()
	if turn != 0 && turn == c.turns[repo] {
		return true
	}
	if c.snapshot(ctx, repo) {
		c.turns[repo] = turn
		return true
	}
	return false
}

// snapshot writes the current working tree to the session's checkpoint ref and
// reports whether it recorded one (false outside a git repo or on any failure).
func (c *checkpointer) snapshot(ctx context.Context, repo string) bool {
	if _, err := c.git(ctx, repo, nil, "rev-parse", "--git-dir"); err != nil {
		return false // not a git repository
	}
	tree, ok := c.worktreeTree(ctx, repo)
	if !ok {
		return false
	}
	args := []string{"commit-tree", tree, "-m", "arbos checkpoint"}
	if parent, err := c.git(ctx, repo, nil, "rev-parse", "--verify", "-q", "HEAD"); err == nil {
		if p := strings.TrimSpace(parent); p != "" {
			args = append(args, "-p", p)
		}
	}
	commit, err := c.git(ctx, repo, gitIdentity, args...)
	if err != nil {
		return false
	}
	if _, err := c.git(ctx, repo, nil, "update-ref", c.sessionRef(ctx), strings.TrimSpace(commit), "-m", "arbos checkpoint"); err != nil {
		return false
	}
	return true
}

// worktreeTree stages the entire working tree into a throwaway index (untracked
// files included, .gitignore respected) and writes it as a tree object, leaving
// the user's real index untouched. It is the shared primitive behind both
// snapshot (which commits the tree) and undo's added-file detection (which diffs
// it against the checkpoint to find files created since).
func (c *checkpointer) worktreeTree(ctx context.Context, repo string) (string, bool) {
	idx, err := os.CreateTemp("", "arbos-ckpt-index-*")
	if err != nil {
		return "", false
	}
	idxPath := idx.Name()
	_ = idx.Close()
	// Git treats an existing zero-byte GIT_INDEX_FILE as a corrupt index. Remove
	// the temp file so git can create and initialize the throwaway index itself.
	if err := os.Remove(idxPath); err != nil {
		return "", false
	}
	defer func() { _ = os.Remove(idxPath) }()

	env := append([]string{"GIT_INDEX_FILE=" + idxPath}, gitIdentity...)
	if _, err := c.git(ctx, repo, env, "add", "-A"); err != nil {
		return "", false
	}
	tree, err := c.git(ctx, repo, env, "write-tree")
	if err != nil {
		return "", false
	}
	return strings.TrimSpace(tree), true
}

// repoForTarget returns the owning git worktree for a target path. For not-yet-
// existing paths it walks up to the nearest existing parent first, so writing
// repo/new/dir/file still checkpoints repo before creating the directories.
func (c *checkpointer) repoForTarget(ctx context.Context, abs string) (string, bool) {
	dir := abs
	if info, err := os.Stat(abs); err != nil || !info.IsDir() {
		dir = filepath.Dir(abs)
	}
	for {
		if _, err := os.Stat(dir); err == nil {
			break
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", false
		}
		dir = parent
	}
	out, err := c.git(ctx, dir, nil, "rev-parse", "--show-toplevel")
	if err != nil {
		return "", false
	}
	repo := strings.TrimSpace(out)
	return repo, repo != ""
}

// sessionRef is the per-session checkpoint ref. Per-session refs give each chat
// or delegated child its own restore timeline, but the snapshot itself is still
// of the shared workspace. Undoing one session restores the workspace to that
// session's last restore point; it does not surgically undo only that session's
// edits.
func (c *checkpointer) sessionRef(ctx context.Context) string {
	sid := "default"
	if cor, ok := obs.From(ctx); ok && cor.SessionID != "" {
		sid = sanitizeRefComponent(cor.SessionID)
	}
	return "refs/arbos/checkpoints/" + sid
}

// sanitizeRefComponent keeps a session id to characters git allows in a ref
// path component, so an exotic id can never produce an invalid ref.
func sanitizeRefComponent(s string) string {
	return strings.Map(func(r rune) rune {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9', r == '-', r == '_', r == '.':
			return r
		default:
			return '-'
		}
	}, s)
}

// git runs a git command in dir and returns its stdout. extraEnv, when non-nil,
// is appended to the process environment (e.g. GIT_INDEX_FILE).
func (c *checkpointer) git(ctx context.Context, dir string, extraEnv []string, args ...string) (string, error) {
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Dir = dir
	if extraEnv != nil {
		cmd.Env = append(os.Environ(), extraEnv...)
	}
	out, err := cmd.Output()
	return string(out), err
}
