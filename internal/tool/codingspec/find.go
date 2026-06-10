package codingspec

import (
	"context"
	"fmt"
	"os/exec"
	"path/filepath"
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

// findSpec searches for files by glob via fd, respecting .gitignore, matching
// pi's find tool. fd is hard-required (clear error if absent); there is no
// auto-download (the broker-and-allowlist posture, D11).
func findSpec(root string) tool.Spec {
	spec := tool.NewSpec("find",
		fmt.Sprintf("Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to %d results or %dKB (whichever is hit first).", findDefaultLimit, DefaultMaxBytes/1024),
		true,
		func(ctx context.Context, a FindArgs) (string, error) {
			searchPath, err := tool.Resolve(root, a.Path)
			if err != nil {
				return "", err
			}
			fdPath, err := exec.LookPath("fd")
			if err != nil {
				return "", fmt.Errorf("fd is not available on PATH; install fd to use find")
			}
			limit := a.Limit
			if limit <= 0 {
				limit = findDefaultLimit
			}

			args := []string{"--glob", "--color=never", "--hidden", "--no-require-git", "--max-results", fmt.Sprint(limit)}
			pattern := a.Pattern
			if strings.Contains(pattern, "/") {
				args = append(args, "--full-path")
				if !strings.HasPrefix(pattern, "/") && !strings.HasPrefix(pattern, "**/") && pattern != "**" {
					pattern = "**/" + pattern
				}
			}
			args = append(args, "--", pattern, searchPath)

			out, err := exec.CommandContext(ctx, fdPath, args...).Output()
			if err != nil {
				if ctx.Err() != nil {
					return "", ctx.Err()
				}
				// fd exits non-zero with no output on no matches in some modes;
				// surface stderr only when there is genuinely no output.
				if ee, ok := err.(*exec.ExitError); ok && len(out) == 0 {
					msg := strings.TrimSpace(string(ee.Stderr))
					if msg == "" {
						msg = "fd failed"
					}
					return "", fmt.Errorf("%s", msg)
				}
			}

			var rel []string
			for _, raw := range strings.Split(string(out), "\n") {
				line := strings.TrimRight(strings.TrimRight(raw, "\r"), " ")
				if line == "" {
					continue
				}
				trailingSlash := strings.HasSuffix(line, "/") || strings.HasSuffix(line, "\\")
				p := line
				if strings.HasPrefix(line, searchPath) {
					p = strings.TrimPrefix(line, searchPath+string(filepath.Separator))
				} else if r, e := filepath.Rel(searchPath, line); e == nil {
					p = r
				}
				p = filepath.ToSlash(p)
				if trailingSlash && !strings.HasSuffix(p, "/") {
					p += "/"
				}
				rel = append(rel, p)
			}
			if len(rel) == 0 {
				return "No files found matching pattern", nil
			}

			limitReached := len(rel) >= limit
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
