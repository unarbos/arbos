package codingspec

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// LsArgs are the arguments to ls.
type LsArgs struct {
	Path  string `json:"path,omitempty" desc:"Directory to list — relative to the working directory, or absolute. Defaults to the working directory."`
	Limit int    `json:"limit,omitempty" desc:"Maximum number of entries to return (default: 500)."`
}

const lsDefaultLimit = 500

// lsSpec lists a directory, sorted case-insensitively, with a "/" suffix on
// directories and a byte/entry cap, matching pi's ls tool.
func lsSpec(root string) tool.Spec {
	spec := tool.NewSpec("ls",
		fmt.Sprintf("List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to %d entries or %dKB (whichever is hit first).", lsDefaultLimit, DefaultMaxBytes/1024),
		true,
		func(_ context.Context, a LsArgs) (string, error) {
			dir, err := tool.Resolve(root, a.Path)
			if err != nil {
				return "", err
			}
			info, err := os.Stat(dir)
			if err != nil {
				return "", fmt.Errorf("Path not found: %s", a.Path)
			}
			if !info.IsDir() {
				return "", fmt.Errorf("Not a directory: %s", a.Path)
			}
			ents, err := os.ReadDir(dir)
			if err != nil {
				return "", fmt.Errorf("Cannot read directory: %s", err)
			}
			limit := a.Limit
			if limit <= 0 {
				limit = lsDefaultLimit
			}
			names := make([]string, 0, len(ents))
			for _, e := range ents {
				names = append(names, e.Name())
			}
			sort.Slice(names, func(i, j int) bool {
				return strings.ToLower(names[i]) < strings.ToLower(names[j])
			})

			results := make([]string, 0, len(names))
			entryLimitReached := false
			for _, name := range names {
				if len(results) >= limit {
					entryLimitReached = true
					break
				}
				suffix := ""
				st, err := os.Stat(filepath.Join(dir, name))
				if err != nil {
					continue // skip entries we cannot stat
				}
				if st.IsDir() {
					suffix = "/"
				}
				results = append(results, name+suffix)
			}
			if len(results) == 0 {
				return "(empty directory)", nil
			}

			tr := TruncateHead(strings.Join(results, "\n"), maxInt, DefaultMaxBytes)
			out := tr.Content
			var notices []string
			if entryLimitReached {
				notices = append(notices, fmt.Sprintf("%d entries limit reached. Use limit=%d for more", limit, limit*2))
			}
			if tr.Truncated {
				notices = append(notices, fmt.Sprintf("%s limit reached", FormatSize(DefaultMaxBytes)))
			}
			if len(notices) > 0 {
				out += "\n\n[" + strings.Join(notices, ". ") + "]"
			}
			return out, nil
		})
	// The listing depends on what lives in the directory, so declare a read of
	// its subtree (keys are hierarchical) and a same-batch write beneath it is
	// ordered, not raced.
	return tool.WithAccess(spec, func(a LsArgs) core.AccessSet {
		return core.AccessSet{Reads: fileKeys(root, a.Path)}
	})
}

const maxInt = int(^uint(0) >> 1)
