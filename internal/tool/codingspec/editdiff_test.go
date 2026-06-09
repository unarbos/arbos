package codingspec

import (
	"strings"
	"testing"
)

// The edit matcher is the one place the plan grants a unit-test exception: its
// edge cases (tiered matching, re-indentation, replaceAll, uniqueness, overlap,
// BOM/CRLF) are impractical to cover purely at runtime. These assert the
// matcher's behavior and its recovery-oriented error strings.

func TestApplyEdits_ExactSingle(t *testing.T) {
	out, applied, err := applyEdits("hello world", []Edit{{OldText: "world", NewText: "go"}}, "f.txt")
	if err != nil || out != "hello go" {
		t.Fatalf("exact replace: out=%q err=%v", out, err)
	}
	if len(applied) != 1 || applied[0].start != 6 {
		t.Fatalf("applied spans = %+v", applied)
	}
}

func TestApplyEdits_UnicodeTolerantWholeLine(t *testing.T) {
	// Smart quotes, en-dash, NBSP, and trailing whitespace in the file all match
	// an ASCII oldText covering the whole line.
	content := "keep\nsay \u201Chi\u201D \u2013 a\u00A0b   \nkeep2\n"
	out, _, err := applyEdits(content, []Edit{{OldText: `say "hi" - a b`, NewText: "X"}}, "f.txt")
	if err != nil {
		t.Fatalf("unicode-tolerant match should succeed: %v", err)
	}
	// Only the matched line changes; surrounding content is untouched byte-for-byte.
	if out != "keep\nX\nkeep2\n" {
		t.Fatalf("unicode-tolerant replace out=%q", out)
	}
}

func TestApplyEdits_FuzzyDoesNotRewriteRestOfFile(t *testing.T) {
	// A fuzzy match must not normalize unrelated parts of the file: the smart
	// quotes on line 1 and the trailing spaces on line 3 survive an edit that
	// fuzzy-matches line 2.
	content := "doc \u201Cquoted\u201D\nvalue \u2013 here\ntrailing   \n"
	out, _, err := applyEdits(content, []Edit{{OldText: "value - here", NewText: "X"}}, "f.txt")
	if err != nil {
		t.Fatalf("fuzzy match should succeed: %v", err)
	}
	if out != "doc \u201Cquoted\u201D\nX\ntrailing   \n" {
		t.Fatalf("fuzzy edit rewrote unrelated content: out=%q", out)
	}
}

func TestApplyEdits_IndentShift(t *testing.T) {
	// oldText written at the wrong indentation matches via a uniform shift, and
	// newText is re-indented to the file's real indentation.
	content := "func f() {\n\tif x {\n\t\treturn 1\n\t}\n}\n"
	out, _, err := applyEdits(content, []Edit{{
		OldText: "if x {\n\treturn 1\n}",
		NewText: "if x {\n\treturn 2\n}",
	}}, "f.go")
	if err != nil {
		t.Fatalf("indent-shifted match should succeed: %v", err)
	}
	if out != "func f() {\n\tif x {\n\t\treturn 2\n\t}\n}\n" {
		t.Fatalf("re-indented replace out=%q", out)
	}
}

func TestApplyEdits_ReplaceAll(t *testing.T) {
	out, applied, err := applyEdits("a foo b foo c foo", []Edit{{OldText: "foo", NewText: "bar", ReplaceAll: true}}, "f.txt")
	if err != nil || out != "a bar b bar c bar" {
		t.Fatalf("replaceAll: out=%q err=%v", out, err)
	}
	if len(applied) != 3 {
		t.Fatalf("replaceAll should report 3 spans, got %d", len(applied))
	}
}

func TestApplyEdits_LineDeletion(t *testing.T) {
	// An empty newText for a whole-line match removes the line, not just its text.
	out, _, err := applyEdits("a\nb\nc\n", []Edit{{OldText: "b\n", NewText: ""}}, "f.txt")
	if err != nil || out != "a\nc\n" {
		t.Fatalf("line deletion: out=%q err=%v", out, err)
	}
}

func TestApplyEdits_NotFound(t *testing.T) {
	_, _, err := applyEdits("abc", []Edit{{OldText: "zzz", NewText: "q"}}, "f.txt")
	if err == nil || !strings.Contains(err.Error(), "Could not find the oldText in f.txt") {
		t.Fatalf("not-found error mismatch: %v", err)
	}
}

func TestApplyEdits_NotFoundNearMiss(t *testing.T) {
	// A near-miss (content matches, indentation differs beyond a uniform shift)
	// surfaces the closest region with its real line numbers so the retry can
	// copy it exactly.
	content := "one\n  alpha\n    beta\nfour\n"
	_, _, err := applyEdits(content, []Edit{{OldText: "alpha\nbeta\nGAMMA", NewText: "x"}}, "f.txt")
	if err == nil {
		t.Fatalf("expected not-found error")
	}
	msg := err.Error()
	if !strings.Contains(msg, "Closest match in the file:") || !strings.Contains(msg, "alpha") || !strings.Contains(msg, "2|") {
		t.Fatalf("near-miss hint missing or unnumbered: %v", msg)
	}
}

func TestApplyEdits_NonUnique(t *testing.T) {
	_, _, err := applyEdits("x\nx\n", []Edit{{OldText: "x", NewText: "y"}}, "f.txt")
	if err == nil {
		t.Fatalf("expected duplicate error")
	}
	msg := err.Error()
	if !strings.Contains(msg, "Found 2 occurrences of the oldText in f.txt (starting at lines 1, 2)") ||
		!strings.Contains(msg, "set replaceAll") {
		t.Fatalf("duplicate error mismatch: %v", msg)
	}
}

func TestApplyEdits_Empty(t *testing.T) {
	_, _, err := applyEdits("abc", []Edit{{OldText: "", NewText: "y"}}, "f.txt")
	if err == nil || err.Error() != "oldText must not be empty in f.txt." {
		t.Fatalf("empty error mismatch: %v", err)
	}
}

func TestApplyEdits_NoChange(t *testing.T) {
	_, _, err := applyEdits("abc", []Edit{{OldText: "abc", NewText: "abc"}}, "f.txt")
	if err == nil || err.Error() != "No changes made to f.txt. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected." {
		t.Fatalf("no-change error mismatch: %v", err)
	}
}

func TestApplyEdits_Overlap(t *testing.T) {
	// Two edits whose matched regions overlap are rejected.
	_, _, err := applyEdits("abcdef", []Edit{
		{OldText: "abcd", NewText: "X"},
		{OldText: "cdef", NewText: "Y"},
	}, "f.txt")
	if err == nil || err.Error() != "edits[0] and edits[1] overlap in f.txt. Merge them into one edit or target disjoint regions." {
		t.Fatalf("overlap error mismatch: %v", err)
	}
}

func TestApplyEdits_MultiDisjointReverseOrder(t *testing.T) {
	// Two disjoint edits apply correctly regardless of source order (reverse-order
	// application keeps offsets valid).
	out, _, err := applyEdits("aaa bbb ccc", []Edit{
		{OldText: "aaa", NewText: "1"},
		{OldText: "ccc", NewText: "3"},
	}, "f.txt")
	if err != nil || out != "1 bbb 3" {
		t.Fatalf("multi-edit out=%q err=%v", out, err)
	}
}

func TestApplyEdits_MultiNonUniqueIndexed(t *testing.T) {
	_, _, err := applyEdits("p\np\nq", []Edit{
		{OldText: "q", NewText: "Q"},
		{OldText: "p", NewText: "P"},
	}, "f.txt")
	if err == nil || !strings.Contains(err.Error(), "Found 2 occurrences of edits[1].oldText in f.txt") {
		t.Fatalf("indexed duplicate error mismatch: %v", err)
	}
}

func TestChangedLineRanges(t *testing.T) {
	// "b" on line 2 becomes two lines; "d" on line 4 is replaced in place. The
	// second range must account for the first replacement's line delta.
	content := "a\nb\nc\nd\n"
	out, applied, err := applyEdits(content, []Edit{
		{OldText: "b\n", NewText: "b1\nb2\n"},
		{OldText: "d\n", NewText: "D\n"},
	}, "f.txt")
	if err != nil || out != "a\nb1\nb2\nc\nD\n" {
		t.Fatalf("setup: out=%q err=%v", out, err)
	}
	ranges := changedLineRanges(content, applied)
	if len(ranges) != 2 || ranges[0] != (lineRange{2, 3}) || ranges[1] != (lineRange{5, 5}) {
		t.Fatalf("changed ranges = %+v", ranges)
	}
}

func TestGenerateDiffString(t *testing.T) {
	old := "a\nb\nc\nd\ne"
	neu := "a\nb\nX\nd\ne"
	got := generateDiffString(old, neu, 1)
	want := " 2 b\n-3 c\n+3 X\n 4 d"
	if got != want {
		t.Fatalf("diff:\n%q\nwant:\n%q", got, want)
	}
	if generateDiffString("same", "same", 3) != "" {
		t.Fatalf("identical content should produce empty diff")
	}
	// Changes at both ends with a large unchanged middle collapse to an ellipsis.
	big := generateDiffString("h\n1\n2\n3\n4\n5\n6\nt", "H\n1\n2\n3\n4\n5\n6\nT", 1)
	if !strings.Contains(big, "...") || !strings.Contains(big, "-1 h") || !strings.Contains(big, "+1 H") {
		t.Fatalf("expected collapsed context with ellipsis, got:\n%s", big)
	}

	// A huge file must short-circuit (the DP matrix would OOM) rather than alloc.
	huge := strings.Repeat("x\n", 4200)
	hugeMod := "y\n" + strings.Repeat("x\n", 4199)
	if got := generateDiffString(huge, hugeMod, 3); got != "(diff omitted: file too large to display)" {
		t.Fatalf("expected large-file diff to be omitted, got %q", got)
	}
}
