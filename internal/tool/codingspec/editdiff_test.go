package codingspec

import (
	"strings"
	"testing"
)

// The edit matcher is the one place the plan grants a unit-test exception: its
// edge cases (fuzzy normalization, uniqueness, overlap, BOM/CRLF) are impractical
// to cover purely at runtime. These assert the pi-faithful behavior and the
// verbatim error strings.

func TestApplyEdits_ExactSingle(t *testing.T) {
	_, out, err := applyEditsToNormalizedContent("hello world", []Edit{{OldText: "world", NewText: "go"}}, "f.txt")
	if err != nil || out != "hello go" {
		t.Fatalf("exact replace: out=%q err=%v", out, err)
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

func TestApplyEdits_FuzzyNormalization(t *testing.T) {
	// Smart quotes, en-dash, NBSP, and trailing whitespace in the file all match
	// an ASCII oldText via the fuzzy fallback.
	content := "say \u201Chi\u201D \u2013 a\u00A0b   \n"
	_, out, err := applyEditsToNormalizedContent(content, []Edit{{OldText: `say "hi" - a b`, NewText: "X"}}, "f.txt")
	if err != nil {
		t.Fatalf("fuzzy match should succeed: %v", err)
	}
	if out != "X\n" {
		t.Fatalf("fuzzy replace out=%q", out)
	}
}

func TestApplyEdits_NotFound(t *testing.T) {
	_, _, err := applyEditsToNormalizedContent("abc", []Edit{{OldText: "zzz", NewText: "q"}}, "f.txt")
	want := "Could not find the exact text in f.txt. The old text must match exactly including all whitespace and newlines."
	if err == nil || err.Error() != want {
		t.Fatalf("not-found error mismatch: %v", err)
	}
}

func TestApplyEdits_NonUnique(t *testing.T) {
	_, _, err := applyEditsToNormalizedContent("x\nx\n", []Edit{{OldText: "x", NewText: "y"}}, "f.txt")
	want := "Found 2 occurrences of the text in f.txt. The text must be unique. Please provide more context to make it unique."
	if err == nil || err.Error() != want {
		t.Fatalf("duplicate error mismatch: %v", err)
	}
}

func TestApplyEdits_Empty(t *testing.T) {
	_, _, err := applyEditsToNormalizedContent("abc", []Edit{{OldText: "", NewText: "y"}}, "f.txt")
	if err == nil || err.Error() != "oldText must not be empty in f.txt." {
		t.Fatalf("empty error mismatch: %v", err)
	}
}

func TestApplyEdits_NoChange(t *testing.T) {
	_, _, err := applyEditsToNormalizedContent("abc", []Edit{{OldText: "abc", NewText: "abc"}}, "f.txt")
	if err == nil || err.Error() != "No changes made to f.txt. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected." {
		t.Fatalf("no-change error mismatch: %v", err)
	}
}

func TestApplyEdits_Overlap(t *testing.T) {
	// Two edits whose matched regions overlap are rejected.
	_, _, err := applyEditsToNormalizedContent("abcdef", []Edit{
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
	_, out, err := applyEditsToNormalizedContent("aaa bbb ccc", []Edit{
		{OldText: "aaa", NewText: "1"},
		{OldText: "ccc", NewText: "3"},
	}, "f.txt")
	if err != nil || out != "1 bbb 3" {
		t.Fatalf("multi-edit out=%q err=%v", out, err)
	}
}

func TestApplyEdits_MultiNonUniqueIndexed(t *testing.T) {
	_, _, err := applyEditsToNormalizedContent("p\np\nq", []Edit{
		{OldText: "q", NewText: "Q"},
		{OldText: "p", NewText: "P"},
	}, "f.txt")
	want := "Found 2 occurrences of edits[1] in f.txt. Each oldText must be unique. Please provide more context to make it unique."
	if err == nil || err.Error() != want {
		t.Fatalf("indexed duplicate error mismatch: %v", err)
	}
}
