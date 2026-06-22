// Blog export: an interactive HTML blog (ADR-0039) is the source of truth;
// markdown is a lossy, derived export. blogToMarkdown walks the document's
// semantic markup — the <article> body's headings, prose, lists, tables,
// blockquotes, code, and citation links — and emits standard markdown. An
// interactive island (data-island) collapses to its data-export fallback text
// (or a fenced chart/mermaid spec where it carried one), so a reader who only
// has the markdown still gets the argument. The export is deliberately not a
// byte-perfect round-trip: a 3D viewer becomes a one-line description.

package gateway

import (
	"strings"

	"golang.org/x/net/html"
	"golang.org/x/net/html/atom"
)

// blogToMarkdown converts a self-contained HTML blog to a markdown export.
// It walks the first <article> (the reading body) when present, else the whole
// document, ignoring <script>/<style>/<head> chrome.
func blogToMarkdown(src string) (string, error) {
	doc, err := html.Parse(strings.NewReader(src))
	if err != nil {
		return "", err
	}
	root := findArticle(doc)
	if root == nil {
		root = doc
	}
	var w mdWriter
	w.block(root)
	return strings.TrimRight(w.b.String(), "\n") + "\n", nil
}

// findArticle returns the first <article> element, or nil.
func findArticle(n *html.Node) *html.Node {
	if n.Type == html.ElementNode && n.DataAtom == atom.Article {
		return n
	}
	for c := n.FirstChild; c != nil; c = c.NextSibling {
		if a := findArticle(c); a != nil {
			return a
		}
	}
	return nil
}

// mdWriter accumulates markdown, separating block elements with blank lines.
type mdWriter struct {
	b       strings.Builder
	pending bool // a block was written; emit a blank line before the next
}

// sep ensures a blank line before the next block (but not at the very start).
func (w *mdWriter) sep() {
	if w.b.Len() > 0 {
		w.b.WriteString("\n\n")
	}
}

// block walks n's children, dispatching block-level elements. Inline content
// encountered directly is gathered into a paragraph.
func (w *mdWriter) block(n *html.Node) {
	for c := n.FirstChild; c != nil; c = c.NextSibling {
		switch {
		case c.Type == html.ElementNode:
			w.element(c)
		case c.Type == html.TextNode && strings.TrimSpace(c.Data) != "":
			// Stray inline text at block level — rare; emit as a paragraph.
			w.sep()
			w.b.WriteString(collapseSpace(c.Data))
		}
	}
}

func (w *mdWriter) element(n *html.Node) {
	// An interactive island collapses to its export fallback regardless of tag.
	if island, ok := attr(n, "data-island"); ok {
		w.island(n, island)
		return
	}
	switch n.DataAtom {
	case atom.Script, atom.Style, atom.Head, atom.Template, atom.Noscript:
		return
	case atom.H1, atom.H2, atom.H3, atom.H4, atom.H5, atom.H6:
		level := int(n.Data[1] - '0')
		w.sep()
		w.b.WriteString(strings.Repeat("#", level) + " " + w.inline(n))
	case atom.P:
		txt := w.inline(n)
		if strings.TrimSpace(txt) == "" {
			return
		}
		w.sep()
		w.b.WriteString(txt)
	case atom.Ul:
		w.list(n, false)
	case atom.Ol:
		w.list(n, true)
	case atom.Blockquote:
		w.sep()
		for _, line := range strings.Split(strings.TrimSpace(w.inline(n)), "\n") {
			w.b.WriteString("> " + line + "\n")
		}
		w.b.WriteString("\x00") // marker removed below; keep trailing handling simple
	case atom.Pre:
		w.sep()
		lang, _ := attr(n, "data-lang")
		w.b.WriteString("```" + lang + "\n" + textContent(n) + "\n```")
	case atom.Table:
		w.table(n)
	case atom.Hr:
		w.sep()
		w.b.WriteString("---")
	case atom.Figure, atom.Section, atom.Div, atom.Header, atom.Footer, atom.Main, atom.Article:
		// Structural wrappers: recurse into their block children.
		w.block(n)
	default:
		// Unknown block-ish element: recurse so nested content isn't lost.
		w.block(n)
	}
	// Normalize the blockquote marker hack.
	if n.DataAtom == atom.Blockquote {
		s := strings.TrimSuffix(w.b.String(), "\x00")
		s = strings.TrimRight(s, "\n")
		w.b.Reset()
		w.b.WriteString(s)
	}
}

// island emits the markdown fallback for an interactive block: its
// data-export text, as a chart/mermaid fence when the kind maps to one, else a
// bracketed note. A <figcaption> (if any) is appended as a caption line.
func (w *mdWriter) island(n *html.Node, kind string) {
	exportText, _ := attr(n, "data-export")
	exportText = strings.TrimSpace(exportText)
	caption := figcaption(n)
	w.sep()
	switch kind {
	case "chart", "mermaid", "diagram":
		// A described figure: a bracketed note carries the description; a
		// caption (which may hold a citation) follows as its own line.
		note := exportText
		if note == "" {
			note = "interactive " + kind
		}
		w.b.WriteString("> **[" + kind + "]** " + note)
		if caption != "" {
			w.b.WriteString("\n>\n> " + caption)
		}
	case "runnable":
		w.b.WriteString("> **[runnable demo]** " + orDefault(exportText, "interactive code demo"))
		if caption != "" {
			w.b.WriteString("\n>\n> " + caption)
		}
	default:
		w.b.WriteString("> **[interactive]** " + orDefault(exportText, kind))
		if caption != "" {
			w.b.WriteString("\n>\n> " + caption)
		}
	}
}

func (w *mdWriter) list(n *html.Node, ordered bool) {
	w.sep()
	idx := 0
	for c := n.FirstChild; c != nil; c = c.NextSibling {
		if c.Type != html.ElementNode || c.DataAtom != atom.Li {
			continue
		}
		idx++
		marker := "- "
		if ordered {
			marker = itoa(idx) + ". "
		}
		w.b.WriteString(marker + strings.TrimSpace(w.inline(c)) + "\n")
	}
	w.b.WriteString("\x00")
	s := strings.TrimSuffix(w.b.String(), "\x00")
	s = strings.TrimRight(s, "\n")
	w.b.Reset()
	w.b.WriteString(s)
}

func (w *mdWriter) table(n *html.Node) {
	var rows [][]string
	var walk func(*html.Node)
	walk = func(x *html.Node) {
		for c := x.FirstChild; c != nil; c = c.NextSibling {
			if c.Type == html.ElementNode && c.DataAtom == atom.Tr {
				var cells []string
				for d := c.FirstChild; d != nil; d = d.NextSibling {
					if d.Type == html.ElementNode && (d.DataAtom == atom.Td || d.DataAtom == atom.Th) {
						cells = append(cells, strings.TrimSpace(w.inline(d)))
					}
				}
				if len(cells) > 0 {
					rows = append(rows, cells)
				}
			} else if c.Type == html.ElementNode {
				walk(c)
			}
		}
	}
	walk(n)
	if len(rows) == 0 {
		return
	}
	w.sep()
	cols := len(rows[0])
	w.b.WriteString("| " + strings.Join(rows[0], " | ") + " |\n")
	w.b.WriteString("|" + strings.Repeat("---|", cols) + "\n")
	for _, r := range rows[1:] {
		for len(r) < cols {
			r = append(r, "")
		}
		w.b.WriteString("| " + strings.Join(r[:cols], " | ") + " |\n")
	}
	s := strings.TrimRight(w.b.String(), "\n")
	w.b.Reset()
	w.b.WriteString(s)
}

// inline renders an element's inline content to markdown: text, emphasis,
// code, links, and citation links (an <a class="cite" href="#cite-src-N">)
// collapse back to the bare [N] marker the /blog convention uses. Runs of
// whitespace fold to a single space, but the space BETWEEN adjacent inline
// nodes is preserved (so "claim <a>[1]</a>" stays "claim [1]", not "claim[1]").
func (w *mdWriter) inline(n *html.Node) string {
	var b strings.Builder
	var walk func(*html.Node)
	walk = func(x *html.Node) {
		for c := x.FirstChild; c != nil; c = c.NextSibling {
			switch {
			case c.Type == html.TextNode:
				b.WriteString(collapseSpaceEdges(c.Data))
			case c.Type == html.ElementNode:
				switch c.DataAtom {
				case atom.Strong, atom.B:
					b.WriteString("**" + strings.TrimSpace(w.inline(c)) + "**")
				case atom.Em, atom.I:
					b.WriteString("*" + strings.TrimSpace(w.inline(c)) + "*")
				case atom.Code:
					b.WriteString("`" + textContent(c) + "`")
				case atom.Br:
					b.WriteString("\n")
				case atom.A:
					if class, _ := attr(c, "class"); strings.Contains(class, "cite") {
						// A citation link: keep its visible [N] text only.
						b.WriteString(collapseSpace(textContent(c)))
						break
					}
					href, _ := attr(c, "href")
					text := strings.TrimSpace(w.inline(c))
					if href == "" || text == href {
						b.WriteString(text)
					} else {
						b.WriteString("[" + text + "](" + href + ")")
					}
				case atom.Img:
					alt, _ := attr(c, "alt")
					src, _ := attr(c, "src")
					b.WriteString("![" + alt + "](" + src + ")")
				case atom.Figcaption, atom.Script, atom.Style:
					// Skip: a figure's caption is handled by island(); scripts
					// never export.
				default:
					walk(c)
				}
			}
		}
	}
	walk(n)
	return strings.TrimSpace(collapseSpace(b.String()))
}

/* ---- small helpers ---- */

func attr(n *html.Node, key string) (string, bool) {
	for _, a := range n.Attr {
		if a.Key == key {
			return a.Val, true
		}
	}
	return "", false
}

// textContent returns the concatenated text of a node (no markup).
func textContent(n *html.Node) string {
	var b strings.Builder
	var walk func(*html.Node)
	walk = func(x *html.Node) {
		for c := x.FirstChild; c != nil; c = c.NextSibling {
			if c.Type == html.TextNode {
				b.WriteString(c.Data)
			} else if c.Type == html.ElementNode && c.DataAtom != atom.Script && c.DataAtom != atom.Style {
				walk(c)
			}
		}
	}
	walk(n)
	return strings.TrimSpace(b.String())
}

// figcaption returns the trimmed text of a descendant <figcaption>, or "".
func figcaption(n *html.Node) string {
	var found string
	var walk func(*html.Node)
	walk = func(x *html.Node) {
		if found != "" {
			return
		}
		for c := x.FirstChild; c != nil; c = c.NextSibling {
			if c.Type == html.ElementNode && c.DataAtom == atom.Figcaption {
				found = collapseSpace(textContent(c))
				return
			}
			walk(c)
		}
	}
	walk(n)
	return strings.TrimSpace(found)
}

// collapseSpace folds runs of whitespace (including newlines) to single spaces
// and trims the ends.
func collapseSpace(s string) string {
	return strings.Join(strings.Fields(s), " ")
}

// collapseSpaceEdges folds interior whitespace runs to single spaces but keeps
// a single leading/trailing space when the original had any — so the boundary
// space between adjacent inline siblings (text, emphasis, links) survives.
func collapseSpaceEdges(s string) string {
	if s == "" {
		return ""
	}
	lead := s[0] == ' ' || s[0] == '\n' || s[0] == '\t' || s[0] == '\r'
	last := s[len(s)-1]
	trail := last == ' ' || last == '\n' || last == '\t' || last == '\r'
	core := strings.Join(strings.Fields(s), " ")
	if core == "" {
		return " " // all whitespace: a single separating space
	}
	if lead {
		core = " " + core
	}
	if trail {
		core += " "
	}
	return core
}

func orDefault(s, def string) string {
	if s == "" {
		return def
	}
	return s
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var digits []byte
	for n > 0 {
		digits = append([]byte{byte('0' + n%10)}, digits...)
		n /= 10
	}
	return string(digits)
}
