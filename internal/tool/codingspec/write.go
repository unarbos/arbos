package codingspec

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"unicode/utf8"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// WriteArgs are the arguments to write.
type WriteArgs struct {
	Path    string `json:"path" desc:"Path to the file to write — relative to the working directory, or absolute (any path on the machine)."`
	Content string `json:"content" desc:"Content to write to the file."`
}

// writeSpec creates or overwrites a file with the given content verbatim,
// creating parent directories, matching pi's write tool. Note: pi's write does
// NOT preserve BOM or line endings (it is a complete rewrite); only edit does.
// The "bytes" count matches pi's content.length (Unicode code points), equal to
// the byte count for ASCII. The new version is noted in the read ledger so
// edit's staleness guard does not misfire on the model's own write.
//
// Staleness guard (symmetric with edit): if the model read or wrote this file
// earlier in the turn and it has since changed on disk (a background job or
// another agent), the overwrite is refused so one actor cannot silently clobber
// another's concurrent write. A brand-new file, or one the model never saw this
// turn, has nothing to be stale against and writes freely.
func writeSpec(root string, ledger *readLedger, cp *checkpointer) tool.Spec {
	spec := tool.NewSpec("write",
		"Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
		false,
		func(ctx context.Context, a WriteArgs) (string, error) {
			abs, err := tool.Resolve(root, a.Path)
			if err != nil {
				return "", err
			}
			key := tool.ResourceKey(abs)
			unlock := lockMutation(key)
			defer unlock()
			turn := turnFromContext(ctx)
			if existing, err := os.ReadFile(abs); err == nil {
				if prev, ok := ledger.peekVersion(turn, key); ok && prev != fileVersion(existing) {
					return "", fmt.Errorf("%s changed on disk since you last saw it this turn, so overwriting it would clobber another change. Use changes to inspect what moved, re-read the file, merge your intended content with the new version, and retry.", a.Path)
				}
			} else if os.IsNotExist(err) {
				if _, ok := ledger.peekVersion(turn, key); ok {
					return "", fmt.Errorf("%s was deleted since you last saw it this turn, so recreating it would clobber another change. Use changes to inspect what moved, then decide whether to leave it deleted or recreate it intentionally.", a.Path)
				}
			} else {
				return "", fmt.Errorf("write: %w", err)
			}
			if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
				return "", fmt.Errorf("write: %w", err)
			}
			undoCovered := cp.ensurePath(ctx, abs)
			if err := os.WriteFile(abs, []byte(a.Content), 0o644); err != nil {
				return "", fmt.Errorf("write: %w", err)
			}
			ledger.noteVersion(turn, key, fileVersion([]byte(a.Content)))
			msg := fmt.Sprintf("Successfully wrote %d bytes to %s", utf8.RuneCountInString(a.Content), a.Path)
			if !undoCovered {
				msg += "\n\nNote: this path is not inside a git repository, so no restore point was recorded for undo."
			}
			return msg, nil
		})
	return tool.WithAccess(spec, func(a WriteArgs) core.AccessSet {
		return writeAccess(root, a.Path)
	})
}

// writeAccess is the footprint of a single-file mutator (write, edit): it writes
// exactly its target path. An unresolvable path is unbounded so the call is
// isolated rather than scheduled as if it touched nothing.
func writeAccess(root, path string) core.AccessSet {
	abs, err := tool.Resolve(root, path)
	if err != nil {
		return core.AccessSet{Unknown: true}
	}
	return core.AccessSet{Writes: []string{tool.ResourceKey(abs)}}
}
