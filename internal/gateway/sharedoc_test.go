package gateway

import (
	"strings"
	"testing"
)

const blogFixture = `# A Report

A lede paragraph with a claim [1] and another [2].

## Findings

Some prose citing two sources [1][3] in a row.

- a bullet [2]
- plain bullet

## Sources

1. First Source — Publisher, https://example.com/a
2. Second **Source** — https://example.com/b
3. [Third Source](https://example.com/c) — 2026
`

func TestParseDocCitations(t *testing.T) {
	c := parseDocCitations(blogFixture)
	if len(c) != 3 {
		t.Fatalf("want 3 citations, got %d: %v", len(c), c)
	}
	if got := c["1"]; got != "First Source — Publisher, https://example.com/a" {
		t.Errorf("cite 1 = %q", got)
	}
	// Bold markup stripped for the tooltip.
	if got := c["2"]; got != "Second Source — https://example.com/b" {
		t.Errorf("cite 2 = %q", got)
	}
	// Inline link reduced to text (url).
	if got := c["3"]; got != "Third Source (https://example.com/c) — 2026" {
		t.Errorf("cite 3 = %q", got)
	}
}

func TestRenderSharedDocCitations(t *testing.T) {
	html := renderSharedDoc("report.md", blogFixture)

	// Bare [1] becomes a superscript citation link with a tooltip.
	if !strings.Contains(html, `<a class="cite" href="#cite-src-1" title="First Source — Publisher, https://example.com/a">[1]</a>`) {
		t.Errorf("citation [1] not rendered as a tooltip link:\n%s", html)
	}
	// Consecutive [1][3] both render.
	if !strings.Contains(html, `href="#cite-src-3"`) {
		t.Error("citation [3] not rendered")
	}
	// The Sources list item carries the matching anchor id.
	if !strings.Contains(html, `<li id="cite-src-1" class="src">`) {
		t.Errorf("source 1 not anchored:\n%s", html)
	}
	// A real link still renders as an anchor, not a citation.
	if !strings.Contains(html, `<a href="https://example.com/c" target="_blank"`) {
		t.Error("inline link in sources not rendered as anchor")
	}
	// Headings render.
	if !strings.Contains(html, "<h1>A Report</h1>") || !strings.Contains(html, "<h2>Findings</h2>") {
		t.Error("headings not rendered")
	}
}

func TestRenderSharedDocNoSources(t *testing.T) {
	// Without a Sources section, bare [5] stays literal text.
	html := renderSharedDoc("note.md", "Just a note [5] here.\n")
	if strings.Contains(html, `class="cite"`) {
		t.Error("[5] should stay literal when there is no Sources list")
	}
	if !strings.Contains(html, "[5]") {
		t.Error("literal [5] missing")
	}
}

func TestRenderSharedDocBlocks(t *testing.T) {
	src := "## Code\n\n" +
		"```go\nfmt.Println(\"hi\")\n```\n\n" +
		"| A | B |\n|---|---|\n| 1 | 2 |\n\n" +
		"Some **bold** and *italic* and `code` and a https://example.com link.\n"
	html := renderSharedDoc("x.md", src)
	for _, want := range []string{
		`<pre class="code"><code>`,
		"fmt.Println(&#34;hi&#34;)",
		"<table>", "<th>A</th>", "<td>1</td>",
		"<strong>bold</strong>", "<em>italic</em>", "<code>code</code>",
		`<a href="https://example.com"`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("missing %q in:\n%s", want, html)
		}
	}
}
