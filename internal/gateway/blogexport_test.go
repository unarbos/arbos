package gateway

import (
	"strings"
	"testing"
)

const blogHTMLFixture = `<!doctype html><html><head><style>body{}</style></head><body>
<article class="article">
  <h1>The Title</h1>
  <p class="lede">A lede with a claim <a class="cite" href="#cite-src-1">[1]</a>.</p>
  <p>Body prose with <strong>bold</strong>, <em>italic</em>, <code>code()</code>,
     a <a href="https://example.com">link</a>, and another cite <a class="cite" href="#cite-src-2">[2]</a>.</p>
  <h2>Findings</h2>
  <ul><li>first point</li><li>second point</li></ul>
  <figure class="island" data-island="chart" data-export="Bar chart: A 70, B 20.">
    <svg></svg>
    <figcaption>Failure rates <a class="cite" href="#cite-src-2">[2]</a>.</figcaption>
  </figure>
  <table>
    <thead><tr><th>Tool</th><th>Score</th></tr></thead>
    <tbody><tr><td>Arbos</td><td>n/a</td></tr><tr><td>Devin</td><td>3/20</td></tr></tbody>
  </table>
  <h2>Sources</h2>
  <ol>
    <li id="cite-src-1" class="src">First Source — Pub, https://example.com/a</li>
    <li id="cite-src-2" class="src">Second Source — https://example.com/b</li>
  </ol>
</article>
<script>console.log("ignored")</script>
</body></html>`

func TestBlogToMarkdown(t *testing.T) {
	md, err := blogToMarkdown(blogHTMLFixture)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	for _, want := range []string{
		"# The Title",
		"A lede with a claim [1].",
		"**bold**", "*italic*", "`code()`", "[link](https://example.com)",
		"and another cite [2].",
		"## Findings",
		"- first point",
		"- second point",
		"**[chart]** Bar chart: A 70, B 20.",
		"Failure rates [2].",
		"| Tool | Score |",
		"| Arbos | n/a |",
		"## Sources",
		"1. First Source — Pub, https://example.com/a",
		"2. Second Source — https://example.com/b",
	} {
		if !strings.Contains(md, want) {
			t.Errorf("export missing %q in:\n%s", want, md)
		}
	}
	// The script never leaks into the export.
	if strings.Contains(md, "ignored") {
		t.Errorf("script content leaked into export:\n%s", md)
	}
	// The live SVG markup never leaks; the island collapsed to its fallback.
	if strings.Contains(md, "<svg") {
		t.Errorf("raw island markup leaked:\n%s", md)
	}
}

func TestBlogToMarkdownNoArticle(t *testing.T) {
	// A document with no <article> falls back to walking the body.
	md, err := blogToMarkdown(`<html><body><h1>Plain</h1><p>Hi.</p></body></html>`)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	if !strings.Contains(md, "# Plain") || !strings.Contains(md, "Hi.") {
		t.Errorf("fallback walk missing content:\n%s", md)
	}
}
