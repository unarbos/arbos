// Package transcript renders kernel tool events into compact, human-readable
// labels. The interactive TUI and the one-shot CLI renderer share it so both
// front-ends describe the agent's actions identically.
package transcript

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/unarbos/arbos/internal/core"
)

// Spinner is the shared animation frame set for in-flight tool activity.
var Spinner = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}

// Tally renders per-tool totals, most-used first, e.g.
// "41 tools · read 30 · ls 6 · find 5 · 2 failed". Empty when nothing ran.
func Tally(total, fails int, counts map[string]int) string {
	if total == 0 {
		return ""
	}
	noun := "tools"
	if total == 1 {
		noun = "tool"
	}
	parts := []string{fmt.Sprintf("%d %s", total, noun)}
	names := make([]string, 0, len(counts))
	for n := range counts {
		names = append(names, n)
	}
	sort.Slice(names, func(i, j int) bool {
		if counts[names[i]] != counts[names[j]] {
			return counts[names[i]] > counts[names[j]]
		}
		return names[i] < names[j]
	})
	for i, n := range names {
		if i == 4 {
			parts = append(parts, "…")
			break
		}
		parts = append(parts, fmt.Sprintf("%s %d", n, counts[n]))
	}
	if fails > 0 {
		parts = append(parts, fmt.Sprintf("%d failed", fails))
	}
	return strings.Join(parts, " · ")
}

// ToolLabel renders a tool call as a short human label, e.g. `read foo.go`,
// falling back to the bare tool name when arguments are absent or unrecognized.
func ToolLabel(name string, args json.RawMessage) string {
	var fields map[string]json.RawMessage
	if json.Unmarshal(args, &fields) != nil {
		return name
	}
	str := func(key string) string {
		raw, ok := fields[key]
		if !ok {
			return ""
		}
		var s string
		if json.Unmarshal(raw, &s) != nil {
			return ""
		}
		return s
	}
	switch name {
	case "read", "write":
		if p := str("path"); p != "" {
			return name + " " + p
		}
	case "edit":
		if p := str("path"); p != "" {
			return name + " " + p
		}
	case "bash":
		if c := str("command"); c != "" {
			return name + " " + truncate(c, 60)
		}
	case "grep":
		if p := str("pattern"); p != "" {
			return name + ` "` + truncate(p, 40) + `"`
		}
	case "find":
		if p := str("pattern"); p != "" {
			return name + " " + truncate(p, 40)
		}
	case "ls":
		if p := str("path"); p != "" {
			return name + " " + p
		}
		return name
	case "fetch":
		if u := str("url"); u != "" {
			return name + " " + truncate(u, 50)
		}
	}
	return name
}

// ToolDone renders a finished tool result with a ✓/✗ status prefix and, on
// failure, the first line of the error.
func ToolDone(label string, res core.ToolResult) string {
	if res.IsError {
		msg := strings.TrimSpace(res.Content)
		if i := strings.IndexByte(msg, '\n'); i >= 0 {
			msg = msg[:i]
		}
		if msg != "" {
			return "✗ " + label + " — " + truncate(msg, 100)
		}
		return "✗ " + label
	}
	return "✓ " + label
}

// DiffOf extracts the display diff a tool attached to its result details
// (edit/write tools), or "" when none. Diffs are the agent's work product and
// deserve rendering everywhere tool results do.
func DiffOf(res core.ToolResult) string {
	if len(res.Details) == 0 {
		return ""
	}
	var d struct {
		Diff string `json:"diff"`
	}
	if json.Unmarshal(res.Details, &d) != nil {
		return ""
	}
	return d.Diff
}

// ToolOutputPreview returns a short preview of exploratory tool output
// (ls/bash/grep/find), similar to how Cursor shows raw listings before the
// summary. Returns nil for tools whose output is not worth previewing.
func ToolOutputPreview(label, content string) []string {
	if !previewableTool(label) {
		return nil
	}
	content = strings.TrimSpace(content)
	if content == "" {
		return nil
	}
	const maxLines = 12
	lines := strings.Split(content, "\n")
	var out []string
	for _, ln := range lines {
		ln = strings.TrimRight(ln, " ")
		if ln == "" {
			continue
		}
		out = append(out, ln)
		if len(out) >= maxLines {
			if len(lines) > maxLines {
				out = append(out, fmt.Sprintf("… (%d more lines)", len(lines)-maxLines))
			}
			break
		}
	}
	return out
}

func previewableTool(label string) bool {
	switch {
	case label == "ls", strings.HasPrefix(label, "ls "):
		return true
	case strings.HasPrefix(label, "bash "):
		return true
	case strings.HasPrefix(label, "grep "):
		return true
	case strings.HasPrefix(label, "find "):
		return true
	default:
		return false
	}
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
