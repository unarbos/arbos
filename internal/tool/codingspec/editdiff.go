package codingspec

import (
	"fmt"
	"sort"
	"strconv"
	"strings"
	"unicode"

	"golang.org/x/text/unicode/norm"
)

// Edit is one targeted replacement.
type Edit struct {
	OldText    string `json:"oldText" desc:"Text for one targeted replacement. It must match exactly one region of the original file unless replaceAll is set, and must not overlap with any other edits[] in the same call."`
	NewText    string `json:"newText" desc:"Replacement text for this targeted edit."`
	ReplaceAll bool   `json:"replaceAll,omitempty" desc:"Replace every occurrence of oldText instead of requiring it to be unique. Use for renames and other repeated identical changes."`
}

// The matcher locates each edit in the original file content and applies
// replacements as pure splices of that content — normalization is used only to
// FIND matches, never to rewrite the file. (This deliberately diverges from
// pi's edit-diff.ts, whose fuzzy path spliced into the normalized whole file,
// rewriting every smart quote and trailing space in the file as collateral.)
//
// Matching tries tiers in order of strictness, taking all matches from the
// first tier that produces any:
//
//  1. exact — byte-for-byte substring search (handles mid-line fragments)
//  2. whitespace-tolerant — whole lines, trailing whitespace ignored, with one
//     uniform leading-indent shift across the block; the shift is re-applied
//     to the replacement text so an indent-drifted oldText still lands at the
//     file's real indentation
//  3. unicode-tolerant — like tier 2, but lines also compare equal under NFKC
//     plus smart-quote/dash/special-space mapping
//
// Uniqueness is enforced per edit unless replaceAll; matched regions must not
// overlap; replacements apply in reverse offset order so positions stay valid.
// Error messages are written for recovery: they carry occurrence line numbers
// or the closest near-miss region so the model's next attempt can succeed
// without a full re-read.

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
// It is used only as a line comparator — its output never reaches the file.
func normalizeForFuzzyMatch(text string) string {
	text = norm.NFKC.String(text)
	lines := strings.Split(text, "\n")
	for i := range lines {
		lines[i] = strings.TrimRightFunc(lines[i], unicode.IsSpace)
	}
	text = strings.Join(lines, "\n")
	return strings.Map(fuzzyMapRune, text)
}

// span is one located occurrence of an edit's oldText, in original-content
// byte coordinates, paired with the replacement text adjusted for that
// occurrence (re-indented when a line-anchored tier matched at a shifted
// indent). Applying a span is always a splice of the original content.
type span struct {
	start, end int
	newText    string
}

// lineSpan is the byte range of one line's content, excluding its '\n'.
type lineSpan struct{ start, end int }

// indexLines computes the byte range of every line in content, mirroring
// splitLF's semantics (a trailing newline yields a final empty line).
func indexLines(content string) []lineSpan {
	spans := make([]lineSpan, 0, strings.Count(content, "\n")+1)
	start := 0
	for i := 0; i < len(content); i++ {
		if content[i] == '\n' {
			spans = append(spans, lineSpan{start, i})
			start = i + 1
		}
	}
	return append(spans, lineSpan{start, len(content)})
}

// findMatches locates every non-overlapping occurrence of e.OldText in
// content, trying tiers strictest-first and returning all matches from the
// first tier that yields any.
func findMatches(content string, fileLines []lineSpan, e Edit) []span {
	if s := exactMatches(content, e); len(s) > 0 {
		return s
	}
	if s := lineTierMatches(content, fileLines, e, func(a, b string) bool { return a == b }); len(s) > 0 {
		return s
	}
	return lineTierMatches(content, fileLines, e, func(a, b string) bool {
		return normalizeForFuzzyMatch(a) == normalizeForFuzzyMatch(b)
	})
}

func exactMatches(content string, e Edit) []span {
	var spans []span
	for from := 0; ; {
		i := strings.Index(content[from:], e.OldText)
		if i < 0 {
			return spans
		}
		start := from + i
		spans = append(spans, span{start: start, end: start + len(e.OldText), newText: e.NewText})
		from = start + len(e.OldText)
	}
}

// indentDelta is the uniform leading-whitespace transform observed between
// oldText's lines and a candidate block's lines: the file is indented deeper
// by add, or shallower by remove. One delta must explain every non-blank line
// of the block, and the same transform re-indents the replacement text.
type indentDelta struct {
	add    string
	remove string
	set    bool
}

func applyDelta(indent string, d indentDelta) string {
	switch {
	case d.add != "":
		return d.add + indent
	case d.remove != "":
		return strings.TrimPrefix(indent, d.remove)
	}
	return indent
}

// splitIndent splits a line into its leading space/tab run and the rest.
func splitIndent(line string) (indent, rest string) {
	i := 0
	for i < len(line) && (line[i] == ' ' || line[i] == '\t') {
		i++
	}
	return line[:i], line[i:]
}

func trimTrail(s string) string { return strings.TrimRightFunc(s, unicode.IsSpace) }

// blockLines splits a block of text into lines for line-anchored matching. A
// trailing newline is the last line's terminator, not an extra empty line.
func blockLines(text string) []string {
	lines := splitLF(text)
	if n := len(lines); n > 0 && lines[n-1] == "" {
		lines = lines[:n-1]
	}
	return lines
}

// matchBlockAt reports whether oldLines match the file lines starting at
// index `at` under the given content comparator, and the indent delta that
// makes them match. Blank lines match any blank line and do not constrain the
// delta; the first non-blank pair fixes the delta and every later non-blank
// pair must conform.
func matchBlockAt(content string, fileLines []lineSpan, oldLines []string, at int, eq func(a, b string) bool) (indentDelta, bool) {
	var d indentDelta
	for k, ol := range oldLines {
		fl := content[fileLines[at+k].start:fileLines[at+k].end]
		if strings.TrimSpace(ol) == "" {
			if strings.TrimSpace(fl) != "" {
				return d, false
			}
			continue
		}
		olIndent, olRest := splitIndent(ol)
		flIndent, flRest := splitIndent(fl)
		if !eq(trimTrail(olRest), trimTrail(flRest)) {
			return d, false
		}
		switch {
		case !d.set:
			switch {
			case strings.HasSuffix(flIndent, olIndent):
				d = indentDelta{add: flIndent[:len(flIndent)-len(olIndent)], set: true}
			case strings.HasSuffix(olIndent, flIndent):
				d = indentDelta{remove: olIndent[:len(olIndent)-len(flIndent)], set: true}
			default:
				return d, false
			}
		case applyDelta(olIndent, d) != flIndent:
			return d, false
		}
	}
	return d, true
}

// lineTierMatches finds all non-overlapping whole-line matches of e.OldText
// under the given line comparator. The matched span covers the block's lines
// without the final '\n'; the replacement is e.NewText re-indented by the
// observed delta. An empty replacement widens the span to consume a newline so
// deleting lines does not leave a blank line behind.
func lineTierMatches(content string, fileLines []lineSpan, e Edit, eq func(a, b string) bool) []span {
	oldLines := blockLines(e.OldText)
	if len(oldLines) == 0 {
		return nil
	}
	newText := strings.TrimSuffix(e.NewText, "\n")
	var spans []span
	for i := 0; i+len(oldLines) <= len(fileLines); {
		d, ok := matchBlockAt(content, fileLines, oldLines, i, eq)
		if !ok {
			i++
			continue
		}
		s := span{start: fileLines[i].start, end: fileLines[i+len(oldLines)-1].end, newText: reindent(newText, d)}
		if s.newText == "" {
			s = widenForDeletion(content, s)
		}
		spans = append(spans, s)
		i += len(oldLines)
	}
	return spans
}

// reindent applies the matched block's indent delta to every non-blank line of
// the replacement, so text written at oldText's indentation lands at the
// file's actual indentation.
func reindent(newText string, d indentDelta) string {
	if !d.set || (d.add == "" && d.remove == "") {
		return newText
	}
	lines := splitLF(newText)
	for i, ln := range lines {
		if strings.TrimSpace(ln) == "" {
			continue
		}
		indent, rest := splitIndent(ln)
		lines[i] = applyDelta(indent, d) + rest
	}
	return strings.Join(lines, "\n")
}

func widenForDeletion(content string, s span) span {
	if s.end < len(content) && content[s.end] == '\n' {
		s.end++
		return s
	}
	if s.start > 0 && content[s.start-1] == '\n' {
		s.start--
	}
	return s
}

// lineAt returns the 1-based line number containing byte offset off.
func lineAt(content string, off int) int {
	return 1 + strings.Count(content[:off], "\n")
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

// editRef names one edit in an error message: "the oldText" for a single-edit
// call, "edits[i].oldText" otherwise.
func editRef(i, total int) string {
	if total == 1 {
		return "the oldText"
	}
	return fmt.Sprintf("edits[%d].oldText", i)
}

// notFoundError reports a miss with the closest plausible region when one
// exists, so the retry can copy the file's real text instead of re-reading.
func notFoundError(path, content string, fileLines []lineSpan, oldText string, i, total int) error {
	if hint := nearestMiss(content, fileLines, oldText); hint != "" {
		return fmt.Errorf("Could not find %s in %s. Closest match in the file:\n%s\nCompare carefully — the difference is likely in whitespace, indentation, or quote characters. Use these exact lines in oldText.", editRef(i, total), path, hint)
	}
	return fmt.Errorf("Could not find %s in %s. Re-read the relevant region and copy it exactly, including whitespace and newlines.", editRef(i, total), path)
}

// duplicateError reports where each occurrence starts so the retry can add
// context from the right place, and points at replaceAll for intentional
// repeats.
func duplicateError(path, content string, spans []span, i, total int) error {
	nums := make([]string, len(spans))
	for k, s := range spans {
		nums[k] = strconv.Itoa(lineAt(content, s.start))
	}
	return fmt.Errorf("Found %d occurrences of %s in %s (starting at lines %s). Add surrounding context to make it unique, or set replaceAll to change every occurrence.", len(spans), editRef(i, total), path, strings.Join(nums, ", "))
}

// nearestMissMaxCells caps the line-comparison work (file lines x oldText
// lines) so the near-miss search cannot dominate a huge file's error path.
const nearestMissMaxCells = 4_000_000

// nearestMissMaxLines caps the rendered hint.
const nearestMissMaxLines = 12

// nearestMiss finds the block whose lines best match oldText under the weakest
// comparator (unicode-normalized, indentation ignored) and renders it with its
// file line numbers. It returns "" when nothing plausible exists or the file
// is too large to scan.
func nearestMiss(content string, fileLines []lineSpan, oldText string) string {
	oldLines := blockLines(oldText)
	if len(oldLines) == 0 || len(fileLines)*len(oldLines) > nearestMissMaxCells {
		return ""
	}
	normOld := make([]string, len(oldLines))
	for i, l := range oldLines {
		_, rest := splitIndent(l)
		normOld[i] = normalizeForFuzzyMatch(rest)
	}
	normFile := make([]string, len(fileLines))
	for i, ls := range fileLines {
		_, rest := splitIndent(content[ls.start:ls.end])
		normFile[i] = normalizeForFuzzyMatch(rest)
	}
	best, bestScore := -1, 0
	for i := 0; i+len(oldLines) <= len(fileLines); i++ {
		score := 0
		for k := range oldLines {
			if normOld[k] != "" && normOld[k] == normFile[i+k] {
				score++
			}
		}
		if score > bestScore {
			best, bestScore = i, score
		}
	}
	if best < 0 {
		return ""
	}
	n := len(oldLines)
	if n > nearestMissMaxLines {
		n = nearestMissMaxLines
	}
	seg := content[fileLines[best].start:fileLines[best+n-1].end]
	return numberLines(seg, best+1)
}

// applyEdits matches every edit against content (LF-normalized original),
// enforces uniqueness unless replaceAll, rejects overlap across all matched
// regions, and applies the replacements in reverse offset order. It returns
// the new content and the applied spans in ascending offset order.
func applyEdits(content string, edits []Edit, path string) (out string, applied []span, err error) {
	normEdits := make([]Edit, len(edits))
	for i, e := range edits {
		normEdits[i] = Edit{OldText: normalizeToLF(e.OldText), NewText: normalizeToLF(e.NewText), ReplaceAll: e.ReplaceAll}
		if normEdits[i].OldText == "" {
			return "", nil, emptyOldTextError(path, i, len(normEdits))
		}
	}

	fileLines := indexLines(content)
	type located struct {
		editIndex int
		span
	}
	var all []located
	for i, e := range normEdits {
		spans := findMatches(content, fileLines, e)
		switch {
		case len(spans) == 0:
			return "", nil, notFoundError(path, content, fileLines, e.OldText, i, len(normEdits))
		case len(spans) > 1 && !e.ReplaceAll:
			return "", nil, duplicateError(path, content, spans, i, len(normEdits))
		}
		for _, s := range spans {
			all = append(all, located{i, s})
		}
	}

	sort.Slice(all, func(a, b int) bool { return all[a].start < all[b].start })
	for i := 1; i < len(all); i++ {
		prev, cur := all[i-1], all[i]
		if prev.end > cur.start {
			return "", nil, fmt.Errorf("edits[%d] and edits[%d] overlap in %s. Merge them into one edit or target disjoint regions.", prev.editIndex, cur.editIndex, path)
		}
	}

	out = content
	for i := len(all) - 1; i >= 0; i-- {
		s := all[i].span
		out = out[:s.start] + s.newText + out[s.end:]
	}
	if out == content {
		return "", nil, noChangeError(path, len(normEdits))
	}
	applied = make([]span, len(all))
	for i, l := range all {
		applied[i] = l.span
	}
	return out, applied, nil
}

// changedLineRanges maps the applied spans (ascending, original coordinates)
// to the 1-based line ranges their replacements occupy in the NEW content —
// the coordinates a follow-up read or edit will see. A pure deletion yields
// the line adjacent to the removed block so the caller can still show context.
func changedLineRanges(content string, applied []span) []lineRange {
	ranges := make([]lineRange, 0, len(applied))
	delta := 0
	for _, s := range applied {
		startLine := lineAt(content, s.start) + delta
		region := content[s.start:s.end]
		oldLines := strings.Count(region, "\n") + 1
		if strings.HasSuffix(region, "\n") {
			oldLines--
		}
		newLines := strings.Count(s.newText, "\n") + 1
		if strings.HasSuffix(s.newText, "\n") {
			newLines--
		}
		if s.newText == "" {
			newLines = 0
		}
		r := lineRange{startLine, startLine + newLines - 1}
		if newLines == 0 {
			r = lineRange{max(startLine-1, 1), startLine}
		}
		ranges = append(ranges, r)
		delta += newLines - oldLines
	}
	return ranges
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
