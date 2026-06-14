// Rendering Markdown to Telegram HTML for the bridge. The model speaks
// Markdown; Telegram's parse_mode=HTML is the robust way to show it as rich
// text (only & < > need escaping, and tags are easy to keep balanced — unlike
// MarkdownV2, whose ~18 reserved characters are brittle under streaming).
//
// The one hard constraint is streaming: the presenter edits a message in place
// as tokens arrive, so it re-renders a PARTIAL snapshot many times before the
// text is complete. Telegram rejects a message whose entities are unbalanced
// (an open <b> with no close, a half-open code fence). mdToHTML therefore
// guarantees balanced output for any input: inline spans are only opened once
// their closing delimiter is present on the same line (an unterminated `**`
// shows literally until its partner streams in), and block constructs (fenced
// code, blockquotes) are force-closed at the end of the snapshot.
package messenger

import "strings"

// mdToHTML converts a (possibly partial) Markdown snapshot into balanced
// Telegram HTML. Telegram supports only a fixed tag set, so unsupported block
// constructs degrade: headings become bold lines and list markers become a
// bullet/number prefix (HTML has no list tags here).
func mdToHTML(src string) string {
	var out strings.Builder
	inFence := false
	fenceFirst := false // the next fence body line is the first (no leading newline)
	fenceClose := ""
	inQuote := false

	openQuoteLine := func() {
		if inQuote {
			out.WriteByte('\n')
			return
		}
		if out.Len() > 0 {
			out.WriteByte('\n')
		}
		out.WriteString("<blockquote>")
		inQuote = true
	}
	closeQuote := func() {
		if inQuote {
			out.WriteString("</blockquote>")
			inQuote = false
		}
	}

	for _, raw := range strings.Split(src, "\n") {
		trimmed := strings.TrimSpace(raw)

		// Fenced code spans lines: emit escaped, track the open/close tags, and
		// force-close at the end if the closing fence hasn't streamed in yet.
		if strings.HasPrefix(trimmed, "```") || strings.HasPrefix(trimmed, "~~~") {
			if inFence {
				out.WriteString(fenceClose)
				inFence, fenceClose = false, ""
				continue
			}
			closeQuote()
			if out.Len() > 0 {
				out.WriteByte('\n')
			}
			if lang := strings.TrimSpace(trimmed[3:]); lang != "" {
				out.WriteString(`<pre><code class="language-` + escapeHTML(lang) + `">`)
				fenceClose = "</code></pre>"
			} else {
				out.WriteString("<pre>")
				fenceClose = "</pre>"
			}
			inFence, fenceFirst = true, true
			continue
		}
		if inFence {
			if fenceFirst {
				fenceFirst = false
			} else {
				out.WriteByte('\n')
			}
			out.WriteString(escapeHTML(raw))
			continue
		}

		// Blockquote: fold a run of "> " lines into one <blockquote>.
		if strings.HasPrefix(trimmed, ">") {
			content := strings.TrimPrefix(strings.TrimPrefix(trimmed, ">"), " ")
			openQuoteLine()
			out.WriteString(inlineHTML(content))
			continue
		}
		closeQuote()

		if out.Len() > 0 {
			out.WriteByte('\n')
		}
		out.WriteString(blockLineHTML(trimmed, raw))
	}

	if inFence {
		out.WriteString(fenceClose)
	}
	closeQuote()
	return out.String()
}

// blockLineHTML renders one non-code, non-quote line: headings become bold,
// list items keep a normalized marker, everything else is inline-styled body.
func blockLineHTML(trimmed, raw string) string {
	if trimmed == "" {
		return ""
	}
	if h := mdHeadingLevel(trimmed); h > 0 {
		return "<b>" + inlineHTML(strings.TrimSpace(trimmed[h:])) + "</b>"
	}
	indent := raw[:len(raw)-len(strings.TrimLeft(raw, " \t"))]
	if marker, rest, ok := mdListMarker(trimmed); ok {
		return indent + marker + " " + inlineHTML(rest)
	}
	return indent + inlineHTML(trimmed)
}

// inlineHTML styles inline spans within one line, escaping all other text. A
// span is only emitted when its closing delimiter is found on the same line,
// so the output is always balanced even mid-stream.
func inlineHTML(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); {
		switch {
		case s[i] == '`':
			if end := strings.IndexByte(s[i+1:], '`'); end >= 0 {
				b.WriteString("<code>" + escapeHTML(s[i+1:i+1+end]) + "</code>")
				i += end + 2
				continue
			}
		case strings.HasPrefix(s[i:], "**"):
			if end := strings.Index(s[i+2:], "**"); end >= 0 {
				b.WriteString("<b>" + inlineHTML(s[i+2:i+2+end]) + "</b>")
				i += end + 4
				continue
			}
		case strings.HasPrefix(s[i:], "~~"):
			if end := strings.Index(s[i+2:], "~~"); end >= 0 {
				b.WriteString("<s>" + inlineHTML(s[i+2:i+2+end]) + "</s>")
				i += end + 4
				continue
			}
		case strings.HasPrefix(s[i:], "||"):
			if end := strings.Index(s[i+2:], "||"); end >= 0 {
				b.WriteString("<tg-spoiler>" + inlineHTML(s[i+2:i+2+end]) + "</tg-spoiler>")
				i += end + 4
				continue
			}
		case s[i] == '*':
			if end := strings.IndexByte(s[i+1:], '*'); end >= 0 {
				b.WriteString("<i>" + inlineHTML(s[i+1:i+1+end]) + "</i>")
				i += end + 2
				continue
			}
		case s[i] == '[':
			if text, url, n, ok := parseLink(s[i:]); ok {
				if safeURL(url) {
					b.WriteString(`<a href="` + escapeURL(url) + `">` + inlineHTML(text) + "</a>")
				} else {
					// An unsupported scheme (e.g. javascript:) makes Telegram
					// reject the whole message with a non-parse error the plain
					// fallback can't catch — keep the link literal instead, so
					// Telegram auto-links any bare URL and nothing is dropped.
					b.WriteString(escapeHTML(s[i : i+n]))
				}
				i += n
				continue
			}
		}
		switch s[i] {
		case '&':
			b.WriteString("&amp;")
		case '<':
			b.WriteString("&lt;")
		case '>':
			b.WriteString("&gt;")
		default:
			b.WriteByte(s[i])
		}
		i++
	}
	return b.String()
}

// safeURL reports whether url uses a scheme Telegram accepts in an href.
// Anything else (an empty href, or javascript:/data: and friends) would make
// the Bot API reject the message outright, so those links are kept literal.
func safeURL(url string) bool {
	for _, scheme := range []string{"http://", "https://", "tg://", "mailto:"} {
		if len(url) >= len(scheme) && strings.EqualFold(url[:len(scheme)], scheme) {
			return true
		}
	}
	return false
}

// parseLink matches a leading "[text](url)" and reports the bytes it spans. A
// missing "]", "(", or ")" means it isn't a link — the "[" is then literal.
func parseLink(s string) (text, url string, n int, ok bool) {
	close := strings.IndexByte(s, ']')
	if close < 0 || close+1 >= len(s) || s[close+1] != '(' {
		return "", "", 0, false
	}
	end := strings.IndexByte(s[close+2:], ')')
	if end < 0 {
		return "", "", 0, false
	}
	return s[1:close], s[close+2 : close+2+end], close + 2 + end + 1, true
}

// mdHeadingLevel returns the ATX heading level (1-6) if line is a heading, else 0.
func mdHeadingLevel(line string) int {
	n := 0
	for n < len(line) && line[n] == '#' {
		n++
	}
	if n >= 1 && n <= 6 && n < len(line) && line[n] == ' ' {
		return n
	}
	return 0
}

// mdListMarker recognizes "- ", "* ", "+ ", and "N. " items, returning a
// normalized bullet ("•" or the kept number) and the item text.
func mdListMarker(line string) (marker, rest string, ok bool) {
	switch {
	case strings.HasPrefix(line, "- "), strings.HasPrefix(line, "* "), strings.HasPrefix(line, "+ "):
		return "•", line[2:], true
	}
	for i := 0; i < len(line); i++ {
		if line[i] >= '0' && line[i] <= '9' {
			continue
		}
		if i > 0 && i+1 < len(line) && line[i] == '.' && line[i+1] == ' ' {
			return line[:i+1], line[i+2:], true
		}
		break
	}
	return "", "", false
}

// escapeHTML escapes the three characters Telegram's HTML parser reserves.
func escapeHTML(s string) string {
	return htmlEscaper.Replace(s)
}

// escapeURL escapes a URL for an href attribute (the three reserved characters
// plus the quote that closes the attribute).
func escapeURL(s string) string {
	return urlEscaper.Replace(s)
}

var (
	htmlEscaper = strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;")
	urlEscaper  = strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;")
)
