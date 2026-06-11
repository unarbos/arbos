package codingspec

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// GrepArgs are the arguments to grep.
type GrepArgs struct {
	Pattern    string `json:"pattern" desc:"Search pattern (regex or literal string)."`
	Path       string `json:"path,omitempty" desc:"Directory or file to search — relative to the working directory, or absolute (default: the working directory)."`
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

// grepMatch is one matched line, produced by either search backend and
// consumed by the shared pi-format renderer.
type grepMatch struct {
	path string
	line int
	text string
}

// grepSpec searches file contents with .gitignore respected, reproducing pi's
// grep tool: the `path:line:` row format, context blocks, and the
// match/byte/line truncation notices. It prefers ripgrep when present (D11's
// performance rationale stands — RE2 in Go cannot match rg's throughput on
// huge trees) and falls back to a native regexp scan over the shared
// gitignore walker, so a machine without rg still has a working grep.
func grepSpec(root string) tool.Spec {
	spec := tool.NewSpec("grep",
		fmt.Sprintf("Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to %d matches or %dKB (whichever is hit first). Long lines are truncated to %d chars.", grepDefaultLimit, DefaultMaxBytes/1024, GrepMaxLineLength),
		true,
		func(ctx context.Context, a GrepArgs) (string, error) {
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

			var matches []grepMatch
			var matchLimitReached bool
			if rgPath, lookErr := exec.LookPath("rg"); lookErr == nil {
				matches, matchLimitReached, err = grepRg(ctx, rgPath, a, searchPath, limit)
			} else {
				matches, matchLimitReached, err = grepNative(ctx, a, searchPath, isDir, limit)
			}
			if err != nil {
				return "", err
			}
			if !matchLimitReached && len(matches) == 0 {
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

// grepRg runs ripgrep with --json, collecting matches with an early stop at
// limit. rg exits 0 (matches), 1 (no matches), 2 (error); only a genuine
// error with no matches collected is fatal.
func grepRg(ctx context.Context, rgPath string, a GrepArgs, searchPath string, limit int) ([]grepMatch, bool, error) {
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
		return nil, false, err
	}
	var stderr strings.Builder
	cmd.Stderr = &stderr
	if err := cmd.Start(); err != nil {
		return nil, false, err
	}

	var matches []grepMatch
	limitReached := false
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
		matches = append(matches, grepMatch{path: ev.Data.Path.Text, line: ev.Data.LineNumber, text: ev.Data.Lines.Text})
		if len(matches) >= limit {
			limitReached = true
			cancel() // stop rg early
			break
		}
	}
	_ = cmd.Wait()
	if ctx.Err() != nil {
		return nil, false, ctx.Err()
	}
	if !limitReached && len(matches) == 0 {
		if msg := strings.TrimSpace(stderr.String()); msg != "" && cmd.ProcessState != nil && cmd.ProcessState.ExitCode() > 1 {
			return nil, false, fmt.Errorf("%s", msg)
		}
	}
	return matches, limitReached, nil
}

// grepNative is the dependency-free fallback when rg is absent: Go's RE2
// regexp scanned line by line over the shared gitignore walker. Both engines
// are RE2-family, so pattern dialects align. Binary files (NUL in the first
// 1KB) are skipped, matching rg. Slower than ripgrep on huge trees, but
// gitignore pruning and the match-limit early stop keep repo-scale searches
// fast.
func grepNative(ctx context.Context, a GrepArgs, searchPath string, isDir bool, limit int) ([]grepMatch, bool, error) {
	expr := a.Pattern
	if a.Literal {
		expr = regexp.QuoteMeta(expr)
	}
	if a.IgnoreCase {
		expr = "(?i)" + expr
	}
	re, err := regexp.Compile(expr)
	if err != nil {
		return nil, false, fmt.Errorf("regex parse error: %s", err)
	}

	// Glob filtering follows gitignore-style semantics like rg: a pattern
	// without '/' matches basenames anywhere; one with '/' matches the
	// root-relative path, floating unless anchored.
	glob := a.Glob
	globFull := strings.Contains(glob, "/")
	if globFull {
		if strings.HasPrefix(glob, "/") {
			glob = strings.TrimPrefix(glob, "/")
		} else if !strings.HasPrefix(glob, "**/") && glob != "**" {
			glob = "**/" + glob
		}
	}
	globMatches := func(rel string) bool {
		if glob == "" {
			return true
		}
		if globFull {
			return matchGlob(glob, rel)
		}
		return matchGlob(glob, path.Base(rel))
	}

	var matches []grepMatch
	scan := func(abs string) error {
		data, err := os.ReadFile(abs)
		if err != nil {
			return nil // unreadable files are skipped, not fatal
		}
		probe := data
		if len(probe) > 1024 {
			probe = probe[:1024]
		}
		if bytes.IndexByte(probe, 0) >= 0 {
			return nil // binary
		}
		lineNo := 0
		for start := 0; start < len(data); {
			lineNo++
			line := data[start:]
			if nl := bytes.IndexByte(line, '\n'); nl >= 0 {
				line = line[:nl]
				start += nl + 1
			} else {
				start = len(data)
			}
			if len(line) > 0 && line[len(line)-1] == '\r' {
				line = line[:len(line)-1]
			}
			if re.Match(line) {
				matches = append(matches, grepMatch{path: abs, line: lineNo, text: string(line)})
				if len(matches) >= limit {
					return errWalkStop
				}
			}
		}
		return nil
	}

	if !isDir {
		_ = scan(searchPath)
		return matches, len(matches) >= limit, nil
	}
	err = walkTree(ctx, searchPath, func(abs, rel string, entIsDir bool) error {
		if entIsDir || !globMatches(rel) {
			return nil
		}
		return scan(abs)
	})
	if err != nil {
		return nil, false, err
	}
	return matches, len(matches) >= limit, nil
}
