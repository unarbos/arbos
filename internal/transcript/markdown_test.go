package transcript

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// plainStyles renders without any ANSI so tests assert on structure (bullets,
// indentation, stripped delimiters) rather than color codes.
func plainStyles() MarkdownStyles {
	s := lipgloss.NewStyle()
	return MarkdownStyles{Heading: s, Bold: s, Italic: s, Code: s, Bullet: s, Body: s}
}

func styleAll(deltas ...string) string {
	m := NewMarkdownStyler(plainStyles())
	var out strings.Builder
	for _, d := range deltas {
		out.WriteString(m.Push(d))
	}
	out.WriteString(m.Flush())
	return out.String()
}

func TestMarkdownHeadingStripsHashes(t *testing.T) {
	if got := styleAll("## Title\n"); got != "Title\n" {
		t.Fatalf("heading: got %q", got)
	}
}

func TestMarkdownBulletNormalized(t *testing.T) {
	if got := styleAll("- one\n"); got != "• one\n" {
		t.Fatalf("bullet: got %q", got)
	}
	if got := styleAll("3. three\n"); got != "3. three\n" {
		t.Fatalf("ordered: got %q", got)
	}
}

func TestMarkdownInlineDelimitersStripped(t *testing.T) {
	got := styleAll("a **b** c `d` e *f*\n")
	if got != "a b c d e f\n" {
		t.Fatalf("inline: got %q", got)
	}
}

func TestMarkdownPartialLineHeldUntilNewline(t *testing.T) {
	m := NewMarkdownStyler(plainStyles())
	if out := m.Push("hello"); out != "" {
		t.Fatalf("partial line should not emit, got %q", out)
	}
	if out := m.Push(" world\n"); out != "hello world\n" {
		t.Fatalf("completed line: got %q", out)
	}
}

func TestMarkdownFlushEmitsTrailingPartial(t *testing.T) {
	m := NewMarkdownStyler(plainStyles())
	m.Push("no newline here")
	if out := m.Flush(); out != "no newline here" {
		t.Fatalf("flush: got %q", out)
	}
}

func TestMarkdownFenceHeldVerbatim(t *testing.T) {
	got := styleAll("```go\n", "x := **not bold**\n", "```\n")
	if !strings.Contains(got, "x := **not bold**") {
		t.Fatalf("fence body should be verbatim, got %q", got)
	}
}

func TestMarkdownSplitTokensReassemble(t *testing.T) {
	// Emphasis delimiters split across deltas must still resolve once the line
	// completes — the whole-line styling is what makes this robust.
	if got := styleAll("a **bo", "ld** b\n"); got != "a bold b\n" {
		t.Fatalf("split tokens: got %q", got)
	}
}

func TestMarkdownIndentationPreserved(t *testing.T) {
	if got := styleAll("    - nested\n"); got != "    • nested\n" {
		t.Fatalf("indent: got %q", got)
	}
}
