package codingspec

import (
	"strings"
	"testing"
)

// The bash truncation helper is the second granted unit-test exception. These
// assert the line cap, the byte cap, and the partial-last-line edge case for the
// tail truncation bash uses.

func TestTruncateTail_LineCap(t *testing.T) {
	var b strings.Builder
	for i := 1; i <= 100; i++ {
		b.WriteString("x\n")
	}
	tr := TruncateTail(b.String(), 10, DefaultMaxBytes)
	if !tr.Truncated || tr.TruncatedBy != "lines" || tr.OutputLines != 10 || tr.TotalLines != 100 {
		t.Fatalf("line cap: %+v", tr)
	}
}

func TestTruncateTail_ByteCap(t *testing.T) {
	// Many short lines, capped by bytes well before the line cap.
	var b strings.Builder
	for i := 0; i < 1000; i++ {
		b.WriteString("0123456789\n")
	}
	tr := TruncateTail(b.String(), DefaultMaxLines, 50)
	if !tr.Truncated || tr.TruncatedBy != "bytes" || tr.OutputBytes > 50 {
		t.Fatalf("byte cap: %+v", tr)
	}
}

func TestTruncateTail_PartialLastLine(t *testing.T) {
	// A single line longer than the byte cap yields a partial (tail) line.
	tr := TruncateTail(strings.Repeat("a", 200), DefaultMaxLines, 50)
	if !tr.Truncated || !tr.LastLinePartial || tr.OutputBytes != 50 {
		t.Fatalf("partial last line: %+v", tr)
	}
}

func TestTruncateTail_NoTruncation(t *testing.T) {
	tr := TruncateTail("a\nb\nc\n", DefaultMaxLines, DefaultMaxBytes)
	if tr.Truncated {
		t.Fatalf("should not truncate: %+v", tr)
	}
}
