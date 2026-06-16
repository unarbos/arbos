// Shared markdown documents: a /blog report (or any .md a show tool opened)
// shared as a ScopeFile link is rendered to a self-contained HTML reading page
// instead of served as raw markdown source. A file share is standalone and
// sandboxed (opaque origin, no cookie), so it can't mount the SPA's React
// renderer — so the render runs here in Go, mirroring the panel's renderer
// closely enough for a report: headings, prose, lists, tables, code, links,
// emphasis, and — the point — clickable reference-style [n] citations that
// resolve against the document's Sources list (hover shows the source, click
// jumps to it). No external requests; the page ships its own styling.

package gateway

import (
	"regexp"
	"strings"
	"time"
)

// isMarkdownExt reports whether a shared file should render as a doc page.
func isMarkdownExt(ext string) bool {
	switch strings.ToLower(ext) {
	case ".md", ".markdown", ".mdown", ".mkd":
		return true
	}
	return false
}

// renderSharedDoc builds the full page for a markdown document.
func renderSharedDoc(title, src string) string {
	cites := parseDocCitations(src)
	var b strings.Builder
	b.WriteString(sharedDocHead)
	b.WriteString(`<title>arbos — ` + esc(title) + `</title></head><body>`)
	b.WriteString(`<article class="doc">`)
	b.WriteString(renderMarkdownBody(src, cites))
	b.WriteString(`</article>`)
	b.WriteString(`<footer class="ftr">Shared via arbos · ` + time.Now().UTC().Format("2006-01-02 15:04 UTC") + `</footer>`)
	b.WriteString(`</body></html>`)
	return b.String()
}

/* ------------------------------------------------------------------ */
/* Citations: scan the document's Sources/References section for the   */
/* numbered list, mapping n → its source line. Mirrors the frontend    */
/* buildCitations in web/src/components/Markdown.tsx.                   */
/* ------------------------------------------------------------------ */

var (
	sourcesHeadingRe = regexp.MustCompile(`(?i)^#{1,6}\s+(sources|references|citations|notes)\b`)
	anyHeadingRe     = regexp.MustCompile(`^#{1,6}\s+`)
	sourceItemRe     = regexp.MustCompile(`^\s*(\d+)[.)]\s+(.*\S)`)
)

// parseDocCitations returns n → plain-text source line for each numbered entry
// under the document's Sources/References heading.
func parseDocCitations(src string) map[string]string {
	out := map[string]string{}
	inSources := false
	for _, line := range strings.Split(src, "\n") {
		if sourcesHeadingRe.MatchString(line) {
			inSources = true
			continue
		}
		if inSources && anyHeadingRe.MatchString(line) {
			inSources = false
			continue
		}
		if !inSources {
			continue
		}
		if m := sourceItemRe.FindStringSubmatch(line); m != nil {
			if _, ok := out[m[1]]; !ok {
				out[m[1]] = stripInlineMarkdown(m[2])
			}
		}
	}
	return out
}

var (
	mdBoldRe = regexp.MustCompile(`\*\*([^*]+)\*\*`)
	mdItalRe = regexp.MustCompile(`\*([^*]+)\*`)
	mdLinkRe = regexp.MustCompile(`\[([^\]]+)\]\(([^)]+)\)`)
)

// stripInlineMarkdown reduces inline markdown to plain text for a tooltip.
func stripInlineMarkdown(s string) string {
	s = mdBoldRe.ReplaceAllString(s, "$1")
	s = mdItalRe.ReplaceAllString(s, "$1")
	s = mdLinkRe.ReplaceAllString(s, "$1 ($2)")
	return strings.TrimSpace(s)
}

func citeAnchor(n string) string { return "cite-src-" + n }

/* ------------------------------------------------------------------ */
/* Block renderer. A small CommonMark-ish subset matching the panel's  */
/* parser: fenced code, ATX headings, hr, bullet/ordered lists, pipe   */
/* tables, and paragraphs. Not a full parser — enough for a report.    */
/* ------------------------------------------------------------------ */

var (
	fenceRe        = regexp.MustCompile("^(`{3,})(\\S*)")
	headingRe      = regexp.MustCompile(`^(#{1,6})\s+(.+)`)
	hrRe           = regexp.MustCompile(`^[-*_]{3,}\s*$`)
	ulRe           = regexp.MustCompile(`^[-*+]\s`)
	olRe           = regexp.MustCompile(`^(\d+)[.)]\s`)
	tableRowRe     = regexp.MustCompile(`^\s*\|.*\|\s*$`)
	tableSepCellRe = regexp.MustCompile(`^:?-{2,}:?$`)
)

func renderMarkdownBody(src string, cites map[string]string) string {
	lines := strings.Split(src, "\n")
	var b strings.Builder
	i := 0
	for i < len(lines) {
		line := lines[i]

		if m := fenceRe.FindStringSubmatch(line); m != nil {
			fenceLen := len(m[1])
			lang := m[2]
			i++
			var code []string
			for i < len(lines) {
				if c := closingFence(lines[i]); c >= fenceLen {
					break
				}
				code = append(code, lines[i])
				i++
			}
			i++ // skip closing fence
			b.WriteString(`<div class="codewrap"><div class="codehd">` + esc(orText(lang)) + `</div>`)
			b.WriteString(`<pre class="code"><code>` + esc(strings.Join(code, "\n")) + `</code></pre></div>`)
			continue
		}

		if m := headingRe.FindStringSubmatch(line); m != nil {
			lvl := len(m[1])
			if lvl > 6 {
				lvl = 6
			}
			tag := "h" + string(rune('0'+lvl))
			b.WriteString("<" + tag + ">" + renderInline(m[2], cites) + "</" + tag + ">")
			i++
			continue
		}

		if hrRe.MatchString(line) {
			b.WriteString("<hr>")
			i++
			continue
		}

		if ulRe.MatchString(line) {
			b.WriteString("<ul>")
			for i < len(lines) && ulRe.MatchString(lines[i]) {
				item := ulRe.ReplaceAllString(lines[i], "")
				b.WriteString(renderListItem(item, cites))
				i++
			}
			b.WriteString("</ul>")
			continue
		}

		if olRe.MatchString(line) {
			b.WriteString("<ol>")
			for i < len(lines) {
				m := olRe.FindStringSubmatch(lines[i])
				if m == nil {
					break
				}
				item := olRe.ReplaceAllString(lines[i], "")
				b.WriteString(renderOrderedItem(m[1], item, cites))
				i++
			}
			b.WriteString("</ol>")
			continue
		}

		if tableRowRe.MatchString(line) && i+1 < len(lines) && isTableSeparator(lines[i+1]) {
			header := splitTableRow(line)
			i += 2
			b.WriteString(`<div class="tablewrap"><table><thead><tr>`)
			for _, h := range header {
				b.WriteString("<th>" + renderInline(h, cites) + "</th>")
			}
			b.WriteString("</tr></thead><tbody>")
			for i < len(lines) && tableRowRe.MatchString(lines[i]) {
				b.WriteString("<tr>")
				for _, c := range splitTableRow(lines[i]) {
					b.WriteString("<td>" + renderInline(c, cites) + "</td>")
				}
				b.WriteString("</tr>")
				i++
			}
			b.WriteString("</tbody></table></div>")
			continue
		}

		if strings.TrimSpace(line) == "" {
			i++
			continue
		}

		// Paragraph: consume until a blank line or a block opener.
		var para []string
		for i < len(lines) && strings.TrimSpace(lines[i]) != "" &&
			(len(para) == 0 || !startsBlock(lines[i], i, lines)) {
			para = append(para, lines[i])
			i++
		}
		if len(para) > 0 {
			b.WriteString("<p>" + renderInline(strings.Join(para, "\n"), cites) + "</p>")
		}
	}
	return b.String()
}

// renderListItem renders a bullet item; an ordered item anchors a Sources entry.
func renderListItem(item string, cites map[string]string) string {
	return "<li>" + renderInline(item, cites) + "</li>"
}

// renderOrderedItem anchors the <li> when its text matches a known source, so a
// [n] citation can scroll to it. Matching is by the source number directly.
func renderOrderedItem(n, item string, cites map[string]string) string {
	if src, ok := cites[n]; ok && stripInlineMarkdown(item) == src {
		return `<li id="` + citeAnchor(n) + `" class="src">` + renderInline(item, cites) + "</li>"
	}
	return "<li>" + renderInline(item, cites) + "</li>"
}

func startsBlock(line string, idx int, lines []string) bool {
	if fenceRe.MatchString(line) || headingRe.MatchString(line) || hrRe.MatchString(line) ||
		ulRe.MatchString(line) || olRe.MatchString(line) {
		return true
	}
	if tableRowRe.MatchString(line) && idx+1 < len(lines) && isTableSeparator(lines[idx+1]) {
		return true
	}
	return false
}

func closingFence(line string) int {
	t := strings.TrimRight(line, " \t")
	n := 0
	for n < len(t) && t[n] == '`' {
		n++
	}
	if n >= 3 && strings.TrimLeft(t, "`") == "" {
		return n
	}
	return 0
}

func isTableSeparator(line string) bool {
	if !tableRowRe.MatchString(line) {
		return false
	}
	cells := splitTableRow(line)
	if len(cells) == 0 {
		return false
	}
	for _, c := range cells {
		if !tableSepCellRe.MatchString(c) {
			return false
		}
	}
	return true
}

func splitTableRow(line string) []string {
	t := strings.TrimSpace(line)
	t = strings.TrimPrefix(t, "|")
	t = strings.TrimSuffix(t, "|")
	parts := strings.Split(t, "|")
	for i := range parts {
		parts[i] = strings.TrimSpace(parts[i])
	}
	return parts
}

func orText(s string) string {
	if s == "" {
		return "text"
	}
	return s
}

/* ------------------------------------------------------------------ */
/* Inline renderer: code, links, bold, italic, bare URLs, and bare     */
/* [n] citation markers. Mirrors web parseInline's priority closely.   */
/* ------------------------------------------------------------------ */

// inlineRe captures, in priority order: 1 inline code; 2 image-alt, 3 image-src;
// 4 link-text, 5 link-href; 6 footnote n; 7 bold; 8 italic; 9 bare URL.
var inlineRe = regexp.MustCompile(
	"(`[^`]+`)" + // 1 code
		`|!\[([^\]]*)\]\(([^)]+)\)` + // 2,3 image (alt, src)
		`|\[([^\]]+)\]\(([^)]+)\)` + // 4,5 link (text, href)
		`|\[(\d{1,4})\]` + // 6 footnote
		`|\*\*([^*]+)\*\*` + // 7 bold
		`|\*([^*]+)\*` + // 8 italic
		`|(https?://[^\s<>)\]]+)`, // 9 bare URL
)

func renderInline(text string, cites map[string]string) string {
	var b strings.Builder
	last := 0
	for _, loc := range inlineRe.FindAllStringSubmatchIndex(text, -1) {
		start, end := loc[0], loc[1]
		if start > last {
			b.WriteString(esc(text[last:start]))
		}
		sub := func(i int) string {
			if loc[2*i] < 0 {
				return ""
			}
			return text[loc[2*i]:loc[2*i+1]]
		}
		switch {
		case sub(1) != "": // code
			b.WriteString(`<code>` + esc(strings.Trim(sub(1), "`")) + `</code>`)
		case sub(3) != "": // image — alt 2, src 3
			b.WriteString(renderImage(sub(2), sub(3)))
		case sub(5) != "": // link — text 4, href 5
			b.WriteString(renderLink(sub(4), sub(5), cites))
		case sub(6) != "": // footnote [n]
			b.WriteString(renderFootnote(sub(6), cites))
		case sub(7) != "": // bold
			b.WriteString(`<strong>` + renderInline(sub(7), cites) + `</strong>`)
		case sub(8) != "": // italic
			b.WriteString(`<em>` + renderInline(sub(8), cites) + `</em>`)
		case sub(9) != "": // bare URL
			b.WriteString(renderLink(sub(9), sub(9), cites))
		}
		last = end
	}
	if last < len(text) {
		b.WriteString(esc(text[last:]))
	}
	return b.String()
}

var safeHrefRe = regexp.MustCompile(`(?i)^(https?:|mailto:)`)

func renderLink(text, href string, cites map[string]string) string {
	href = strings.TrimSpace(href)
	// A bare URL's text is the URL itself; render it literally rather than
	// re-parsing (which would re-match the URL and recurse forever).
	inner := esc(text)
	if text != href {
		inner = renderInline(text, cites)
	}
	if !safeHrefRe.MatchString(href) {
		return inner
	}
	return `<a href="` + esc(href) + `" target="_blank" rel="noreferrer noopener">` + inner + `</a>`
}

var safeImgRe = regexp.MustCompile(`(?i)^(https?:|data:image/)`)

func renderImage(alt, src string) string {
	src = strings.TrimSpace(src)
	if !safeImgRe.MatchString(src) {
		return esc(alt)
	}
	return `<img class="img" src="` + esc(src) + `" alt="` + esc(alt) + `">`
}

// renderFootnote turns a bare [n] into a superscript citation link when it
// resolves against the Sources list; otherwise it stays literal text.
func renderFootnote(n string, cites map[string]string) string {
	src, ok := cites[n]
	if !ok {
		return esc("[" + n + "]")
	}
	return `<a class="cite" href="#` + citeAnchor(n) + `" title="` + esc(src) + `">[` + esc(n) + `]</a>`
}

// sharedDocHead is the page shell: doctype, meta, and self-contained styling
// matching the trajectory page's palette so a shared doc looks like arbos. No
// external requests. The closing </head> is appended by renderSharedDoc after
// the <title>.
const sharedDocHead = `<!doctype html><html><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  :root {
    --canvas:#2c262a; --card:#332a2f; --panel:#312b2f; --bar:#262024; --line:#3e373c;
    --text:#d6d0d4; --bright:#ece7ea; --muted:#968f94; --faint:#6e676c; --accent:#85aab3;
  }
  * { box-sizing: border-box; }
  body {
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Inter,Roboto,sans-serif;
    font-size:15px; line-height:1.7; color-scheme:dark; -webkit-font-smoothing:antialiased;
    background:var(--canvas); color:var(--text); margin:0;
  }
  ::selection { background: color-mix(in srgb, var(--accent) 28%, transparent); }
  .doc { max-width: 46rem; margin: 0 auto; padding: 2.6rem 1.2rem 1rem; }
  .doc h1 { font-size:1.9rem; line-height:1.25; color:var(--bright); margin:.2em 0 .5em; font-weight:700; }
  .doc h2 { font-size:1.4rem; color:var(--bright); margin:1.8em 0 .5em; font-weight:650;
    border-bottom:1px solid var(--line); padding-bottom:.25em; }
  .doc h3 { font-size:1.15rem; color:var(--bright); margin:1.4em 0 .4em; font-weight:600; }
  .doc h4, .doc h5, .doc h6 { font-size:1rem; color:var(--bright); margin:1.2em 0 .3em; font-weight:600; }
  .doc p { margin:.7em 0; }
  .doc a { color:var(--accent); text-decoration:none; }
  .doc a:hover { text-decoration:underline; }
  .doc ul, .doc ol { margin:.6em 0; padding-left:1.6em; }
  .doc li { margin:.25em 0; }
  .doc li.src { scroll-margin-top:1rem; }
  .doc li.src:target { background: color-mix(in srgb, var(--accent) 16%, transparent);
    border-radius:5px; outline:1px solid color-mix(in srgb, var(--accent) 40%, transparent);
    outline-offset:3px; }
  .doc hr { border:0; border-top:1px solid var(--line); margin:2em 0; }
  .doc strong { color:var(--bright); font-weight:650; }
  .doc code { background: color-mix(in srgb, var(--bright) 8%, transparent);
    border-radius:4px; padding:.08em .35em; font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.85em; color:var(--bright); }
  a.cite { font-size:.7em; vertical-align:super; font-weight:600; text-decoration:none;
    padding:0 .05em; }
  a.cite:hover { text-decoration:underline; }
  .codewrap { border:1px solid var(--line); border-radius:8px; overflow:hidden; margin:1em 0; }
  .codehd { background:var(--card); color:var(--faint); font-size:11px; padding:.3rem .7rem;
    border-bottom:1px solid var(--line); }
  pre.code { margin:0; padding:.7rem .8rem; overflow-x:auto; background:var(--bar);
    font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:12.5px; line-height:1.5; }
  pre.code code { background:none; padding:0; color:var(--text); font-size:inherit; }
  .tablewrap { overflow-x:auto; margin:1em 0; }
  .doc table { border-collapse:collapse; font-size:.92em; }
  .doc th, .doc td { border:1px solid var(--line); padding:.4em .7em; text-align:left; vertical-align:top; }
  .doc th { color:var(--bright); font-weight:600; background:var(--card); }
  .doc img.img { max-width:100%; border:1px solid var(--line); border-radius:6px; margin:.6em 0; }
  .ftr { max-width:46rem; margin:2rem auto 0; padding:1.2rem; color:var(--faint); font-size:11.5px;
    text-align:center; border-top:1px solid var(--line); }
</style>
`
