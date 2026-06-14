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
				out.WriteString(`<pre><code class="language-` + escapeAttr(lang) + `">`)
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
			// A blockquote may carry bold/italic etc. but not a code span or a
			// link, so those are rendered without their tags inside it.
			out.WriteString(inlineHTML(content, false, false))
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
		// A heading is a <b> wrapper, so its content can't carry a code span.
		return "<b>" + inlineHTML(strings.TrimSpace(trimmed[h:]), false, true) + "</b>"
	}
	indent := raw[:len(raw)-len(strings.TrimLeft(raw, " \t"))]
	if marker, rest, ok := mdListMarker(trimmed); ok {
		return indent + marker + " " + inlineHTML(rest, true, true)
	}
	return indent + inlineHTML(trimmed, true, true)
}

// inlineHTML styles inline spans within one line, escaping all other text. A
// span is only emitted when its closing delimiter is found on the same line,
// so the output is always balanced even mid-stream.
//
// allowCode and allowLink encode Telegram's entity-nesting rules: <code>/<pre>
// may not sit inside any other entity, and <a>/<blockquote> may not contain a
// link. When a span would violate that, its content is rendered without the
// offending tag rather than emitted as HTML Telegram would reject. bold,
// italic, strikethrough, and spoiler may nest freely, so they always recurse.
func inlineHTML(s string, allowCode, allowLink bool) string {
	var b strings.Builder
	for i := 0; i < len(s); {
		switch {
		case s[i] == '`':
			if end := strings.IndexByte(s[i+1:], '`'); end >= 0 {
				code := escapeHTML(s[i+1 : i+1+end])
				if allowCode {
					b.WriteString("<code>" + code + "</code>")
				} else {
					b.WriteString(code) // code can't nest in another entity
				}
				i += end + 2
				continue
			}
		case strings.HasPrefix(s[i:], "**"):
			if end := strings.Index(s[i+2:], "**"); end >= 0 {
				b.WriteString("<b>" + inlineHTML(s[i+2:i+2+end], false, allowLink) + "</b>")
				i += end + 4
				continue
			}
		case strings.HasPrefix(s[i:], "~~"):
			if end := strings.Index(s[i+2:], "~~"); end >= 0 {
				b.WriteString("<s>" + inlineHTML(s[i+2:i+2+end], false, allowLink) + "</s>")
				i += end + 4
				continue
			}
		case strings.HasPrefix(s[i:], "||"):
			if end := strings.Index(s[i+2:], "||"); end >= 0 {
				b.WriteString("<tg-spoiler>" + inlineHTML(s[i+2:i+2+end], false, allowLink) + "</tg-spoiler>")
				i += end + 4
				continue
			}
		case s[i] == '*':
			if end := strings.IndexByte(s[i+1:], '*'); end >= 0 {
				b.WriteString("<i>" + inlineHTML(s[i+1:i+1+end], false, allowLink) + "</i>")
				i += end + 2
				continue
			}
		case s[i] == '[':
			if text, url, n, ok := parseLink(s[i:]); ok {
				if allowLink && safeURL(url) {
					// A link can't contain a link or a code span.
					b.WriteString(`<a href="` + escapeURL(url) + `">` + inlineHTML(text, false, false) + "</a>")
				} else {
					// An unsupported scheme (e.g. javascript:), or a link where
					// one isn't allowed (inside a link or a blockquote), would
					// make Telegram reject the message with an error the plain
					// fallback can't catch — keep it literal so Telegram still
					// auto-links any bare URL and nothing is dropped.
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
	closeIdx := strings.IndexByte(s, ']')
	if closeIdx < 0 || closeIdx+1 >= len(s) || s[closeIdx+1] != '(' {
		return "", "", 0, false
	}
	end := strings.IndexByte(s[closeIdx+2:], ')')
	if end < 0 {
		return "", "", 0, false
	}
	return s[1:closeIdx], s[closeIdx+2 : closeIdx+2+end], closeIdx + 2 + end + 1, true
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
	return attrEscaper.Replace(s)
}

// escapeAttr escapes an attribute value (e.g. a code fence's language): the
// reserved characters plus the quote that would otherwise close the attribute.
func escapeAttr(s string) string {
	return attrEscaper.Replace(s)
}

var (
	htmlEscaper = strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;")
	attrEscaper = strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;")
)
