package codingspec

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"unicode/utf8"

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
// the byte count for ASCII.
func writeSpec(root string) tool.Spec {
	return tool.NewSpec("write",
		"Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
		false,
		func(_ context.Context, a WriteArgs) (string, error) {
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
			return fmt.Sprintf("Successfully wrote %d bytes to %s", utf8.RuneCountInString(a.Content), a.Path), nil
		})
}
