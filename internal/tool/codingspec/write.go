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
	Path    string `json:"path" desc:"Path to the file to write, relative to the workspace root."`
	Content string `json:"content" desc:"Content to write to the file."`
}

// writeSpec creates or overwrites a file with the given content verbatim,
// creating parent directories, matching pi's write tool. Note: pi's write does
// NOT preserve BOM or line endings (it is a complete rewrite); only edit does.
// The "bytes" count matches pi's content.length (Unicode code points), equal to
// the byte count for ASCII. The new version is noted in the read ledger so
// edit's staleness guard does not misfire on the model's own write.
func writeSpec(root string, ledger *readLedger) tool.Spec {
	spec := tool.NewSpec("write",
		"Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
		false,
		func(ctx context.Context, a WriteArgs) (string, error) {
			abs, err := tool.Resolve(root, a.Path)
			if err != nil {
				return "", err
			}
			if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
				return "", fmt.Errorf("write: %w", err)
			}
			if err := os.WriteFile(abs, []byte(a.Content), 0o644); err != nil {
				return "", fmt.Errorf("write: %w", err)
			}
			ledger.noteVersion(turnFromContext(ctx), abs, fileVersion([]byte(a.Content)))
			return fmt.Sprintf("Successfully wrote %d bytes to %s", utf8.RuneCountInString(a.Content), a.Path), nil
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
	return core.AccessSet{Writes: []string{abs}}
}
