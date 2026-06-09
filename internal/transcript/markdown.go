package transcript

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// MarkdownStyles is the palette a MarkdownStyler paints with. Front-ends supply
// styles bound to their own output stream so color degrades correctly when the
// stream is not a terminal.
type MarkdownStyles struct {
	Heading lipgloss.Style // # headings
	Bold    lipgloss.Style // **strong**
	Italic  lipgloss.Style // *emphasis*
	Code    lipgloss.Style // `inline` and fenced blocks
	Bullet  lipgloss.Style // list markers and blockquote bars
	Body    lipgloss.Style // everything else
}

// MarkdownStyler styles assistant prose a line at a time. Prose arrives as
// streaming token deltas, so a `**` or a ``` fence is rarely complete within a
// single delta; styling whole lines (which are inline-complete once their
// trailing newline lands) keeps the live token feel while turning raw Markdown
// into colored, indented output instead of a literal dump.
//
// A styler is single-use per assistant message. Feed deltas with Push and call
// Flush once the message ends to style any trailing partial line.
type MarkdownStyler struct {
	st      MarkdownStyles
	buf     strings.Builder // current in-progress line, sans newline
	inFence bool            // inside a ``` fenced code block
}

// NewMarkdownStyler returns a styler painting with st.
func NewMarkdownStyler(st MarkdownStyles) *MarkdownStyler {
	return &MarkdownStyler{st: st}
}

// Push consumes a prose delta and returns the styled text ready to print. Only
// newline-completed lines are styled and emitted; the trailing partial line is
// held until the next newline or a Flush, so the caller prints complete styled
// lines and never a half-formatted one.
func (m *MarkdownStyler) Push(delta string) string {
	if delta == "" {
		return ""
	}
	m.buf.WriteString(delta)
	full := m.buf.String()
	nl := strings.LastIndexByte(full, '\n')
	if nl < 0 {
		return ""
	}
	complete := full[:nl+1]
	m.buf.Reset()
	m.buf.WriteString(full[nl+1:])

	var out strings.Builder
	for _, line := range strings.SplitAfter(complete, "\n") {
		if line == "" {
			continue
		}
		nl := strings.HasSuffix(line, "\n")
		out.WriteString(m.styleLine(strings.TrimSuffix(line, "\n")))
		if nl {
			out.WriteByte('\n')
		}
	}
	return out.String()
}

// Flush styles and returns any buffered partial line (no trailing newline).
// Call it once when the assistant message ends.
func (m *MarkdownStyler) Flush() string {
	if m.buf.Len() == 0 {
		return ""
	}
	line := m.buf.String()
	m.buf.Reset()
	return m.styleLine(line)
}

// styleLine renders one complete line of Markdown. Block context (headings,
// lists, quotes, fences) is decided here; inline spans within body text are
// handled by styleInline.
func (m *MarkdownStyler) styleLine(line string) string {
	trimmed := strings.TrimSpace(line)

	if strings.HasPrefix(trimmed, "```") || strings.HasPrefix(trimmed, "~~~") {
		m.inFence = !m.inFence
		return m.st.Bullet.Render(line)
	}
	if m.inFence {
		return m.st.Code.Render(line)
	}

	indent := line[:len(line)-len(strings.TrimLeft(line, " \t"))]

	if h := headingLevel(trimmed); h > 0 {
		return indent + m.st.Heading.Render(strings.TrimSpace(trimmed[h:]))
	}
	if strings.HasPrefix(trimmed, "> ") {
		return indent + m.st.Bullet.Render("│ ") + m.styleInline(trimmed[2:])
	}
	if marker, rest, ok := listMarker(trimmed); ok {
		return indent + m.st.Bullet.Render(marker) + " " + m.styleInline(rest)
	}
	return indent + m.styleInline(trimmed)
}

// headingLevel returns the ATX heading level (1-6) if line is a heading, else 0.
func headingLevel(line string) int {
	n := 0
	for n < len(line) && line[n] == '#' {
		n++
	}
	if n >= 1 && n <= 6 && n < len(line) && line[n] == ' ' {
		return n
	}
	return 0
}

// listMarker recognizes "- ", "* ", "+ ", and "N. " list items, returning a
// normalized bullet ("•" or the kept number) and the item text.
func listMarker(line string) (marker, rest string, ok bool) {
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

// styleInline colors inline spans: `code`, **bold**, and *italic*. It scans
// once, applying the first matching delimiter, so styles do not nest but the
// common cases read correctly.
func (m *MarkdownStyler) styleInline(s string) string {
	var out, plain strings.Builder
	flush := func() {
		if plain.Len() > 0 {
			out.WriteString(m.st.Body.Render(plain.String()))
			plain.Reset()
		}
	}
	for i := 0; i < len(s); {
		switch {
		case s[i] == '`':
			if end := strings.IndexByte(s[i+1:], '`'); end >= 0 {
				flush()
				out.WriteString(m.st.Code.Render(s[i+1 : i+1+end]))
				i += end + 2
				continue
			}
		case strings.HasPrefix(s[i:], "**"):
			if end := strings.Index(s[i+2:], "**"); end >= 0 {
				flush()
				out.WriteString(m.st.Bold.Render(s[i+2 : i+2+end]))
				i += end + 4
				continue
			}
		case s[i] == '*':
			if end := strings.IndexByte(s[i+1:], '*'); end >= 0 {
				flush()
				out.WriteString(m.st.Italic.Render(s[i+1 : i+1+end]))
				i += end + 2
				continue
			}
		}
		plain.WriteByte(s[i])
		i++
	}
	flush()
	return out.String()
}
