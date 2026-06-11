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

// ToolVerb maps a tool name to a short gerund phrase for the live activity
// line — "reading files", "running commands" — so the spinner region reads as
// what the agent is doing rather than a churn of raw tool calls and their
// output. Unknown tools (including MCP `server__tool` names) fall back to the
// bare name.
func ToolVerb(name string) string {
	switch name {
	case "read":
		return "reading files"
	case "ls":
		return "listing files"
	case "find":
		return "finding files"
	case "grep":
		return "searching"
	case "write":
		return "writing files"
	case "edit":
		return "editing files"
	case "bash":
		return "running commands"
	case "fetch":
		return "fetching"
	case "await":
		return "waiting on jobs"
	case "jobs":
		return "checking jobs"
	case "plan":
		return "updating the plan"
	case "delegate":
		return "delegating"
	default:
		return name
	}
}

// DiffStat counts added and removed lines in a display diff (lines beginning
// with + or -), for the compact "(+a −b)" summary the renderers show in place
// of dumping the whole diff into the terminal.
func DiffStat(diff string) (add, del int) {
	for _, ln := range strings.Split(diff, "\n") {
		switch {
		case strings.HasPrefix(ln, "+"):
			add++
		case strings.HasPrefix(ln, "-"):
			del++
		}
	}
	return add, del
}

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
	case "delegate":
		if inst := str("instruction"); inst != "" {
			line, _, _ := strings.Cut(strings.TrimSpace(inst), "\n")
			return truncate(line, 60)
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

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
