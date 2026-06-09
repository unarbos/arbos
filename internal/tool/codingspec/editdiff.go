package codingspec

import (
	"fmt"
	"sort"
	"strconv"
	"strings"
	"unicode"

	"golang.org/x/text/unicode/norm"
)

// Edit is one exact-text replacement.
type Edit struct {
	OldText string `json:"oldText" desc:"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."`
	NewText string `json:"newText" desc:"Replacement text for this targeted edit."`
}

// The matcher below is a faithful port of pi's edit-diff.ts: exact match first,
// then a fuzzy fallback over NFKC-normalized, trailing-whitespace-stripped,
// quote/dash/space-normalized content. Uniqueness is enforced, overlapping edits
// are rejected, and replacements apply in reverse offset order. The error
// strings are reproduced verbatim because the model reads them.

func normalizeToLF(s string) string {
	return strings.ReplaceAll(strings.ReplaceAll(s, "\r\n", "\n"), "\r", "\n")
}

func detectLineEnding(content string) string {
	crlf := strings.Index(content, "\r\n")
	lf := strings.Index(content, "\n")
	if lf == -1 || crlf == -1 {
		return "\n"
	}
	if crlf < lf {
		return "\r\n"
	}
	return "\n"
}

func restoreLineEndings(text, ending string) string {
	if ending == "\r\n" {
		return strings.ReplaceAll(text, "\n", "\r\n")
	}
	return text
}

func stripBom(content string) (bom, text string) {
	if strings.HasPrefix(content, "\uFEFF") {
		return "\uFEFF", content[len("\uFEFF"):]
	}
	return "", content
}

// fuzzyMapRune normalizes smart quotes, Unicode dashes, and special spaces to
// their ASCII equivalents, matching pi's normalizeForFuzzyMatch replacements.
func fuzzyMapRune(r rune) rune {
	switch {
	case r == '\u2018' || r == '\u2019' || r == '\u201A' || r == '\u201B':
		return '\''
	case r == '\u201C' || r == '\u201D' || r == '\u201E' || r == '\u201F':
		return '"'
	case r == '\u2010' || r == '\u2011' || r == '\u2012' || r == '\u2013' || r == '\u2014' || r == '\u2015' || r == '\u2212':
		return '-'
	case r == '\u00A0' || (r >= '\u2002' && r <= '\u200A') || r == '\u202F' || r == '\u205F' || r == '\u3000':
		return ' '
	}
	return r
}

// normalizeForFuzzyMatch applies, in pi's exact order: NFKC, per-line
// trailing-whitespace strip, then smart-quote / dash / special-space mapping.
func normalizeForFuzzyMatch(text string) string {
	text = norm.NFKC.String(text)
	lines := strings.Split(text, "\n")
	for i := range lines {
		lines[i] = strings.TrimRightFunc(lines[i], unicode.IsSpace)
	}
	text = strings.Join(lines, "\n")
	return strings.Map(fuzzyMapRune, text)
}

type fuzzyMatch struct {
	found                 bool
	index                 int // byte offset
	matchLength           int // bytes
	usedFuzzyMatch        bool
	contentForReplacement string
}

func fuzzyFindText(content, oldText string) fuzzyMatch {
	if i := strings.Index(content, oldText); i != -1 {
		return fuzzyMatch{found: true, index: i, matchLength: len(oldText), contentForReplacement: content}
	}
	fc := normalizeForFuzzyMatch(content)
	fo := normalizeForFuzzyMatch(oldText)
	if i := strings.Index(fc, fo); i != -1 {
		return fuzzyMatch{found: true, index: i, matchLength: len(fo), usedFuzzyMatch: true, contentForReplacement: fc}
	}
	return fuzzyMatch{}
}

func countOccurrences(content, oldText string) int {
	return strings.Count(normalizeForFuzzyMatch(content), normalizeForFuzzyMatch(oldText))
}

func notFoundError(path string, i, total int) error {
	if total == 1 {
		return fmt.Errorf("Could not find the exact text in %s. The old text must match exactly including all whitespace and newlines.", path)
	}
	return fmt.Errorf("Could not find edits[%d] in %s. The oldText must match exactly including all whitespace and newlines.", i, path)
}

func duplicateError(path string, i, total, occ int) error {
	if total == 1 {
		return fmt.Errorf("Found %d occurrences of the text in %s. The text must be unique. Please provide more context to make it unique.", occ, path)
	}
	return fmt.Errorf("Found %d occurrences of edits[%d] in %s. Each oldText must be unique. Please provide more context to make it unique.", occ, i, path)
}

func emptyOldTextError(path string, i, total int) error {
	if total == 1 {
		return fmt.Errorf("oldText must not be empty in %s.", path)
	}
	return fmt.Errorf("edits[%d].oldText must not be empty in %s.", i, path)
}

func noChangeError(path string, total int) error {
	if total == 1 {
		return fmt.Errorf("No changes made to %s. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.", path)
	}
	return fmt.Errorf("No changes made to %s. The replacements produced identical content.", path)
}

type matchedEdit struct {
	editIndex   int
	matchIndex  int
	matchLength int
	newText     string
}

// applyEditsToNormalizedContent matches every edit against the original content
// (exact then fuzzy), enforces uniqueness, rejects overlap, and applies the
// replacements in reverse offset order so positions stay valid. It returns the
// base content (fuzzy-normalized if any edit needed fuzzy matching) and the new
// content.
func applyEditsToNormalizedContent(normalizedContent string, edits []Edit, path string) (base, out string, err error) {
	norm := make([]Edit, len(edits))
	for i, e := range edits {
		norm[i] = Edit{OldText: normalizeToLF(e.OldText), NewText: normalizeToLF(e.NewText)}
		if norm[i].OldText == "" {
			return "", "", emptyOldTextError(path, i, len(norm))
		}
	}

	usedFuzzy := false
	for _, e := range norm {
		if fuzzyFindText(normalizedContent, e.OldText).usedFuzzyMatch {
			usedFuzzy = true
		}
	}
	base = normalizedContent
	if usedFuzzy {
		base = normalizeForFuzzyMatch(normalizedContent)
	}

	matched := make([]matchedEdit, 0, len(norm))
	for i, e := range norm {
		m := fuzzyFindText(base, e.OldText)
		if !m.found {
			return "", "", notFoundError(path, i, len(norm))
		}
		if occ := countOccurrences(base, e.OldText); occ > 1 {
			return "", "", duplicateError(path, i, len(norm), occ)
		}
		matched = append(matched, matchedEdit{editIndex: i, matchIndex: m.index, matchLength: m.matchLength, newText: e.NewText})
	}

	sort.Slice(matched, func(a, b int) bool { return matched[a].matchIndex < matched[b].matchIndex })
	for i := 1; i < len(matched); i++ {
		prev, cur := matched[i-1], matched[i]
		if prev.matchIndex+prev.matchLength > cur.matchIndex {
			return "", "", fmt.Errorf("edits[%d] and edits[%d] overlap in %s. Merge them into one edit or target disjoint regions.", prev.editIndex, cur.editIndex, path)
		}
	}

	out = base
	for i := len(matched) - 1; i >= 0; i-- {
		m := matched[i]
		out = out[:m.matchIndex] + m.newText + out[m.matchIndex+m.matchLength:]
	}
	if base == out {
		return "", "", noChangeError(path, len(norm))
	}
	return base, out, nil
}

// firstChangedLine returns the 1-indexed line in the new content where it first
// diverges from the old content, or 0 if identical.
func firstChangedLine(oldContent, newContent string) int {
	o := strings.Split(oldContent, "\n")
	n := strings.Split(newContent, "\n")
	for i := 0; i < len(o) && i < len(n); i++ {
		if o[i] != n[i] {
			return i + 1
		}
	}
	if len(o) != len(n) {
		return min(len(o), len(n)) + 1
	}
	return 0
}

type diffOp struct {
	kind rune // ' ' equal, '+' added, '-' removed
	text string
}

// diffLineOps computes a line-level diff of old vs new via an LCS backtrace,
// yielding a flat op list (equal/added/removed) in output order.
func diffLineOps(a, b []string) []diffOp {
	n, m := len(a), len(b)
	dp := make([][]int, n+1)
	for i := range dp {
		dp[i] = make([]int, m+1)
	}
	for i := n - 1; i >= 0; i-- {
		for j := m - 1; j >= 0; j-- {
			if a[i] == b[j] {
				dp[i][j] = dp[i+1][j+1] + 1
			} else if dp[i+1][j] >= dp[i][j+1] {
				dp[i][j] = dp[i+1][j]
			} else {
				dp[i][j] = dp[i][j+1]
			}
		}
	}
	var ops []diffOp
	i, j := 0, 0
	for i < n && j < m {
		switch {
		case a[i] == b[j]:
			ops = append(ops, diffOp{' ', a[i]})
			i++
			j++
		case dp[i+1][j] >= dp[i][j+1]:
			ops = append(ops, diffOp{'-', a[i]})
			i++
		default:
			ops = append(ops, diffOp{'+', b[j]})
			j++
		}
	}
	for ; i < n; i++ {
		ops = append(ops, diffOp{'-', a[i]})
	}
	for ; j < m; j++ {
		ops = append(ops, diffOp{'+', b[j]})
	}
	return ops
}

// generateDiffString renders a line-numbered display diff (pi's edit diff view):
// changed lines marked +/- with the new/old line number, surrounded by up to
// contextLines unchanged lines, with long unchanged runs collapsed to "...".
// Context lines and removals number against the old file; additions against new.
// maxDiffCells caps the LCS dynamic-programming matrix at ~16M ints (~128 MB)
// so a large-file edit cannot OOM the process. The diff is display-only (it
// rides Details, which the model never sees), so omitting it on huge files costs
// nothing but the rendered view.
const maxDiffCells = 16_000_000

func generateDiffString(oldContent, newContent string, contextLines int) string {
	if oldContent == newContent {
		return ""
	}
	oldLines, newLines := splitLF(oldContent), splitLF(newContent)
	if int64(len(oldLines)+1)*int64(len(newLines)+1) > maxDiffCells {
		return "(diff omitted: file too large to display)"
	}
	ops := diffLineOps(oldLines, newLines)

	// Mark which ops are within contextLines of a change.
	keep := make([]bool, len(ops))
	for idx, op := range ops {
		if op.kind == ' ' {
			continue
		}
		for d := -contextLines; d <= contextLines; d++ {
			k := idx + d
			if k >= 0 && k < len(ops) {
				keep[k] = true
			}
		}
	}

	// Compute number width from the largest line number we may print.
	maxNum := len(oldLines)
	if len(newLines) > maxNum {
		maxNum = len(newLines)
	}
	width := len(strconv.Itoa(maxNum))
	if width < 1 {
		width = 1
	}

	ellipsis := strings.Repeat(" ", width+1) + "..."
	var rows []string
	oldNum, newNum := 0, 0
	skipping := false
	for idx, op := range ops {
		switch op.kind {
		case ' ':
			oldNum++
			newNum++
		case '-':
			oldNum++
		case '+':
			newNum++
		}
		if !keep[idx] {
			if !skipping {
				rows = append(rows, ellipsis)
				skipping = true
			}
			continue
		}
		skipping = false
		switch op.kind {
		case ' ':
			rows = append(rows, fmt.Sprintf(" %*d %s", width, oldNum, op.text))
		case '-':
			rows = append(rows, fmt.Sprintf("-%*d %s", width, oldNum, op.text))
		case '+':
			rows = append(rows, fmt.Sprintf("+%*d %s", width, newNum, op.text))
		}
	}
	// Drop leading/trailing collapsed-context markers; keep only interior gaps.
	for len(rows) > 0 && rows[0] == ellipsis {
		rows = rows[1:]
	}
	for len(rows) > 0 && rows[len(rows)-1] == ellipsis {
		rows = rows[:len(rows)-1]
	}
	return strings.Join(rows, "\n")
}
