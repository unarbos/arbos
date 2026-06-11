package codingspec

import (
	"context"
	"fmt"
	"os"
	"path"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// FindArgs are the arguments to find.
type FindArgs struct {
	Pattern string `json:"pattern" desc:"Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'."`
	Path    string `json:"path,omitempty" desc:"Directory to search in — relative to the working directory, or absolute (default: the working directory)."`
	Limit   int    `json:"limit,omitempty" desc:"Maximum number of results (default: 1000)."`
}

const findDefaultLimit = 1000

// findSpec searches for files by glob with a native gitignore-aware walker,
// matching pi's find tool. It used to shell out to fd (D11), but a missing
// binary broke basic navigation on clean machines; a tree walk + glob needs
// none of ripgrep's regex machinery, so find is self-contained while grep
// keeps its rg requirement.
func findSpec(root string) tool.Spec {
	spec := tool.NewSpec("find",
		fmt.Sprintf("Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to %d results or %dKB (whichever is hit first).", findDefaultLimit, DefaultMaxBytes/1024),
		true,
		func(ctx context.Context, a FindArgs) (string, error) {
			searchPath, err := tool.Resolve(root, a.Path)
			if err != nil {
				return "", err
			}
			info, err := os.Stat(searchPath)
			if err != nil {
				return "", fmt.Errorf("Path not found: %s", a.Path)
			}
			if !info.IsDir() {
				return "", fmt.Errorf("Not a directory: %s", a.Path)
			}
			limit := a.Limit
			if limit <= 0 {
				limit = findDefaultLimit
			}

			// A bare-name pattern matches against filenames; one with '/'
			// matches the path relative to the search directory, floating
			// (**/ prefixed) unless anchored with a leading '/'.
			pattern := a.Pattern
			fullPath := strings.Contains(pattern, "/")
			if fullPath {
				if strings.HasPrefix(pattern, "/") {
					pattern = strings.TrimPrefix(pattern, "/")
				} else if !strings.HasPrefix(pattern, "**/") && pattern != "**" {
					pattern = "**/" + pattern
				}
			}
			if !validGlob(pattern) {
				return "", fmt.Errorf("Invalid glob pattern: %s", a.Pattern)
			}

			rel, limitReached, err := findWalk(ctx, searchPath, pattern, fullPath, limit)
			if err != nil {
				return "", err
			}
			if len(rel) == 0 {
				return "No files found matching pattern", nil
			}

			tr := TruncateHead(strings.Join(rel, "\n"), maxInt, DefaultMaxBytes)
			res := tr.Content
			var notices []string
			if limitReached {
				notices = append(notices, fmt.Sprintf("%d results limit reached. Use limit=%d for more, or refine pattern", limit, limit*2))
			}
			if tr.Truncated {
				notices = append(notices, fmt.Sprintf("%s limit reached", FormatSize(DefaultMaxBytes)))
			}
			if len(notices) > 0 {
				res += "\n\n[" + strings.Join(notices, ". ") + "]"
			}
			return res, nil
		})
	// A scan reads its whole search subtree (keys are hierarchical), so a
	// same-batch write beneath it is ordered, not raced.
	return tool.WithAccess(spec, func(a FindArgs) core.AccessSet {
		return core.AccessSet{Reads: fileKeys(root, a.Path)}
	})
}

// findWalk matches names (or root-relative paths when fullPath) against
// pattern over the shared gitignore-aware walk, returning slash-separated
// relative matches (directories suffixed "/") in sorted order up to limit.
// An all-lowercase pattern matches case-insensitively (fd-style smart case).
func findWalk(ctx context.Context, searchPath, pattern string, fullPath bool, limit int) ([]string, bool, error) {
	caseFold := pattern == strings.ToLower(pattern)
	match := func(s string) bool {
		if caseFold {
			s = strings.ToLower(s)
		}
		return matchGlob(pattern, s)
	}

	var results []string
	err := walkTree(ctx, searchPath, func(_, rel string, isDir bool) error {
		target := rel
		if !fullPath {
			target = path.Base(rel)
		}
		if !match(target) {
			return nil
		}
		out := rel
		if isDir {
			out += "/"
		}
		results = append(results, out)
		if len(results) >= limit {
			return errWalkStop
		}
		return nil
	})
	if err != nil {
		return nil, false, err
	}
	return results, len(results) >= limit, nil
}
