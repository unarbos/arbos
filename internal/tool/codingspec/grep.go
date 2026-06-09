package codingspec

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// GrepArgs are the arguments to grep.
type GrepArgs struct {
	Pattern    string `json:"pattern" desc:"Search pattern (regex or literal string)."`
	Path       string `json:"path,omitempty" desc:"Directory or file to search, relative to the workspace root (default: root)."`
	Glob       string `json:"glob,omitempty" desc:"Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'."`
	IgnoreCase bool   `json:"ignoreCase,omitempty" desc:"Case-insensitive search (default: false)."`
	Literal    bool   `json:"literal,omitempty" desc:"Treat pattern as a literal string instead of a regex (default: false)."`
	Context    int    `json:"context,omitempty" desc:"Lines of context to show before and after each match (default: 0)."`
	Limit      int    `json:"limit,omitempty" desc:"Maximum number of matches to return (default: 100)."`
}

const grepDefaultLimit = 100

type rgEvent struct {
	Type string `json:"type"`
	Data struct {
		Path struct {
			Text string `json:"text"`
		} `json:"path"`
		Lines struct {
			Text string `json:"text"`
		} `json:"lines"`
		LineNumber int `json:"line_number"`
	} `json:"data"`
}

// grepSpec searches file contents via ripgrep with .gitignore respected,
// reproducing pi's grep tool: rg --json parsing, the `path:line:` row format,
// context blocks, and the match/byte/line truncation notices. rg is
// hard-required (D11).
func grepSpec(root string) tool.Spec {
	spec := tool.NewSpec("grep",
		fmt.Sprintf("Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to %d matches or %dKB (whichever is hit first). Long lines are truncated to %d chars.", grepDefaultLimit, DefaultMaxBytes/1024, GrepMaxLineLength),
		true,
		func(ctx context.Context, a GrepArgs) (string, error) {
			rgPath, err := exec.LookPath("rg")
			if err != nil {
				return "", fmt.Errorf("ripgrep (rg) is not available on PATH; install ripgrep to use grep")
			}
			searchPath, err := tool.Resolve(root, a.Path)
			if err != nil {
				return "", err
			}
			info, err := os.Stat(searchPath)
			if err != nil {
				return "", fmt.Errorf("Path not found: %s", a.Path)
			}
			isDir := info.IsDir()
			limit := a.Limit
			if limit <= 0 {
				limit = grepDefaultLimit
			}

			args := []string{"--json", "--line-number", "--color=never", "--hidden"}
			if a.IgnoreCase {
				args = append(args, "--ignore-case")
			}
			if a.Literal {
				args = append(args, "--fixed-strings")
			}
			if a.Glob != "" {
				args = append(args, "--glob", a.Glob)
			}
			args = append(args, "--", a.Pattern, searchPath)

			cctx, cancel := context.WithCancel(ctx)
			defer cancel()
			cmd := exec.CommandContext(cctx, rgPath, args...)
			stdout, err := cmd.StdoutPipe()
			if err != nil {
				return "", err
			}
			var stderr strings.Builder
			cmd.Stderr = &stderr
			if err := cmd.Start(); err != nil {
				return "", err
			}

			type match struct {
				path string
				line int
				text string
			}
			var matches []match
			matchLimitReached := false
			sc := bufio.NewScanner(stdout)
			sc.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
			for sc.Scan() {
				line := sc.Bytes()
				if len(line) == 0 {
					continue
				}
				var ev rgEvent
				if json.Unmarshal(line, &ev) != nil || ev.Type != "match" {
					continue
				}
				matches = append(matches, match{path: ev.Data.Path.Text, line: ev.Data.LineNumber, text: ev.Data.Lines.Text})
				if len(matches) >= limit {
					matchLimitReached = true
					cancel() // stop rg early
					break
				}
			}
			_ = cmd.Wait()
			if ctx.Err() != nil {
				return "", ctx.Err()
			}
			// rg exits 0 (matches), 1 (no matches), 2 (error). Only a genuine error
			// with no matches collected is fatal.
			if !matchLimitReached && len(matches) == 0 {
				if msg := strings.TrimSpace(stderr.String()); msg != "" && cmd.ProcessState != nil && cmd.ProcessState.ExitCode() > 1 {
					return "", fmt.Errorf("%s", msg)
				}
				return "No matches found", nil
			}

			fileCache := map[string][]string{}
			getLines := func(p string) []string {
				if l, ok := fileCache[p]; ok {
					return l
				}
				b, err := os.ReadFile(p)
				if err != nil {
					fileCache[p] = nil
					return nil
				}
				l := strings.Split(strings.ReplaceAll(strings.ReplaceAll(string(b), "\r\n", "\n"), "\r", "\n"), "\n")
				fileCache[p] = l
				return l
			}
			relPath := func(p string) string {
				if isDir {
					if r, err := filepath.Rel(searchPath, p); err == nil && !strings.HasPrefix(r, "..") {
						return filepath.ToSlash(r)
					}
				}
				return filepath.Base(p)
			}

			linesTruncated := false
			var out []string
			for _, m := range matches {
				if a.Context <= 0 {
					sanitized := strings.TrimSuffix(strings.ReplaceAll(strings.ReplaceAll(m.text, "\r\n", "\n"), "\r", ""), "\n")
					t, tr := TruncateLine(sanitized, GrepMaxLineLength)
					if tr {
						linesTruncated = true
					}
					out = append(out, fmt.Sprintf("%s:%d: %s", relPath(m.path), m.line, t))
					continue
				}
				lines := getLines(m.path)
				rp := relPath(m.path)
				if len(lines) == 0 {
					out = append(out, fmt.Sprintf("%s:%d: (unable to read file)", rp, m.line))
					continue
				}
				start := m.line - a.Context
				if start < 1 {
					start = 1
				}
				end := m.line + a.Context
				if end > len(lines) {
					end = len(lines)
				}
				for cur := start; cur <= end; cur++ {
					raw := strings.ReplaceAll(lines[cur-1], "\r", "")
					t, tr := TruncateLine(raw, GrepMaxLineLength)
					if tr {
						linesTruncated = true
					}
					if cur == m.line {
						out = append(out, fmt.Sprintf("%s:%d: %s", rp, cur, t))
					} else {
						out = append(out, fmt.Sprintf("%s-%d- %s", rp, cur, t))
					}
				}
			}

			trunc := TruncateHead(strings.Join(out, "\n"), maxInt, DefaultMaxBytes)
			result := trunc.Content
			var notices []string
			if matchLimitReached {
				notices = append(notices, fmt.Sprintf("%d matches limit reached. Use limit=%d for more, or refine pattern", limit, limit*2))
			}
			if trunc.Truncated {
				notices = append(notices, fmt.Sprintf("%s limit reached", FormatSize(DefaultMaxBytes)))
			}
			if linesTruncated {
				notices = append(notices, fmt.Sprintf("Some lines truncated to %d chars. Use read tool to see full lines", GrepMaxLineLength))
			}
			if len(notices) > 0 {
				result += "\n\n[" + strings.Join(notices, ". ") + "]"
			}
			return result, nil
		})
	// A scan reads its whole search subtree (keys are hierarchical), so a
	// same-batch write beneath it is ordered, not raced.
	return tool.WithAccess(spec, func(a GrepArgs) core.AccessSet {
		return core.AccessSet{Reads: fileKeys(root, a.Path)}
	})
}
