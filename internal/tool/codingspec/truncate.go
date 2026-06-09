package codingspec

import (
	"fmt"
	"strings"
	"unicode/utf8"
)

// Truncation utilities ported from pi's coding-agent (tools/truncate.ts).
// Two independent limits, whichever is hit first: a line cap and a byte cap.
// read uses head truncation (keep the beginning), bash uses tail truncation
// (keep the end), grep/find/ls use byte-only head truncation. Byte counts are
// UTF-8 byte lengths, which is exactly len() on a Go string.

const (
	DefaultMaxLines   = 2000
	DefaultMaxBytes   = 50 * 1024
	GrepMaxLineLength = 500
	maxImageBytes     = 20 * 1024 * 1024 // 20 MiB raw; base64 expansion is handled downstream
)

// TruncationResult mirrors pi's TruncationResult so the model-facing notices can
// be reconstructed exactly.
type TruncationResult struct {
	Content               string
	Truncated             bool
	TruncatedBy           string // "lines" | "bytes" | ""
	TotalLines            int
	TotalBytes            int
	OutputLines           int
	OutputBytes           int
	LastLinePartial       bool
	FirstLineExceedsLimit bool
	MaxLines              int
	MaxBytes              int
}

func splitLinesForCounting(content string) []string {
	if len(content) == 0 {
		return nil
	}
	lines := splitLF(content)
	if content[len(content)-1] == '\n' {
		lines = lines[:len(lines)-1]
	}
	return lines
}

// splitLF splits on '\n' without dropping a trailing empty field (Go's
// strings.Split already keeps it; this is a named helper for intent).
func splitLF(s string) []string {
	out := []string{}
	start := 0
	for i := 0; i < len(s); i++ {
		if s[i] == '\n' {
			out = append(out, s[start:i])
			start = i + 1
		}
	}
	out = append(out, s[start:])
	return out
}

// FormatSize renders a byte count the way pi does.
func FormatSize(bytes int) string {
	switch {
	case bytes < 1024:
		return fmt.Sprintf("%dB", bytes)
	case bytes < 1024*1024:
		return fmt.Sprintf("%.1fKB", float64(bytes)/1024)
	default:
		return fmt.Sprintf("%.1fMB", float64(bytes)/(1024*1024))
	}
}

// TruncateHead keeps the first lines/bytes that fit. Never returns a partial
// line; if the first line alone exceeds the byte limit it returns empty content
// with FirstLineExceedsLimit set.
func TruncateHead(content string, maxLines, maxBytes int) TruncationResult {
	if maxLines <= 0 {
		maxLines = DefaultMaxLines
	}
	if maxBytes <= 0 {
		maxBytes = DefaultMaxBytes
	}
	totalBytes := len(content)
	lines := splitLinesForCounting(content)
	totalLines := len(lines)

	if totalLines <= maxLines && totalBytes <= maxBytes {
		return TruncationResult{Content: content, TotalLines: totalLines, TotalBytes: totalBytes,
			OutputLines: totalLines, OutputBytes: totalBytes, MaxLines: maxLines, MaxBytes: maxBytes}
	}
	if totalLines > 0 && len(lines[0]) > maxBytes {
		return TruncationResult{Truncated: true, TruncatedBy: "bytes", TotalLines: totalLines,
			TotalBytes: totalBytes, FirstLineExceedsLimit: true, MaxLines: maxLines, MaxBytes: maxBytes}
	}

	out := make([]string, 0, maxLines)
	outBytes := 0
	truncatedBy := "lines"
	for i := 0; i < len(lines) && i < maxLines; i++ {
		lineBytes := len(lines[i])
		if i > 0 {
			lineBytes++ // newline
		}
		if outBytes+lineBytes > maxBytes {
			truncatedBy = "bytes"
			break
		}
		out = append(out, lines[i])
		outBytes += lineBytes
	}
	if len(out) >= maxLines && outBytes <= maxBytes {
		truncatedBy = "lines"
	}
	content2 := strings.Join(out, "\n")
	return TruncationResult{Content: content2, Truncated: true, TruncatedBy: truncatedBy,
		TotalLines: totalLines, TotalBytes: totalBytes, OutputLines: len(out), OutputBytes: len(content2),
		MaxLines: maxLines, MaxBytes: maxBytes}
}

// TruncateTail keeps the last lines/bytes that fit; the bash tool wants the end
// (errors, final results). May return a partial first line when the final line
// alone exceeds the byte limit.
func TruncateTail(content string, maxLines, maxBytes int) TruncationResult {
	if maxLines <= 0 {
		maxLines = DefaultMaxLines
	}
	if maxBytes <= 0 {
		maxBytes = DefaultMaxBytes
	}
	totalBytes := len(content)
	lines := splitLinesForCounting(content)
	totalLines := len(lines)

	if totalLines <= maxLines && totalBytes <= maxBytes {
		return TruncationResult{Content: content, TotalLines: totalLines, TotalBytes: totalBytes,
			OutputLines: totalLines, OutputBytes: totalBytes, MaxLines: maxLines, MaxBytes: maxBytes}
	}

	out := []string{}
	outBytes := 0
	truncatedBy := "lines"
	lastLinePartial := false
	for i := len(lines) - 1; i >= 0 && len(out) < maxLines; i-- {
		lineBytes := len(lines[i])
		if len(out) > 0 {
			lineBytes++ // newline
		}
		if outBytes+lineBytes > maxBytes {
			truncatedBy = "bytes"
			if len(out) == 0 {
				tl := truncateStringToBytesFromEnd(lines[i], maxBytes)
				out = append([]string{tl}, out...)
				outBytes = len(tl)
				lastLinePartial = true
			}
			break
		}
		out = append([]string{lines[i]}, out...)
		outBytes += lineBytes
	}
	if len(out) >= maxLines && outBytes <= maxBytes {
		truncatedBy = "lines"
	}
	content2 := strings.Join(out, "\n")
	return TruncationResult{Content: content2, Truncated: true, TruncatedBy: truncatedBy,
		TotalLines: totalLines, TotalBytes: totalBytes, OutputLines: len(out), OutputBytes: len(content2),
		LastLinePartial: lastLinePartial, MaxLines: maxLines, MaxBytes: maxBytes}
}

// truncateStringToBytesFromEnd keeps the last maxBytes bytes of s, snapping to a
// valid UTF-8 boundary so a multi-byte rune is never split.
func truncateStringToBytesFromEnd(s string, maxBytes int) string {
	if len(s) <= maxBytes {
		return s
	}
	start := len(s) - maxBytes
	for start < len(s) && (s[start]&0xc0) == 0x80 {
		start++
	}
	return s[start:]
}

// TruncateLine caps a single line to maxChars runes, adding a suffix. Used for
// grep match lines.
func TruncateLine(line string, maxChars int) (string, bool) {
	if maxChars <= 0 {
		maxChars = GrepMaxLineLength
	}
	if utf8.RuneCountInString(line) <= maxChars {
		return line, false
	}
	r := []rune(line)
	return string(r[:maxChars]) + "... [truncated]", true
}
