package codingspec

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func mustJSON(v any) json.RawMessage {
	b, err := json.Marshal(v)
	if err != nil {
		panic(fmt.Sprintf("marshal: %v", err))
	}
	return b
}

// A newline-terminated file must report its real line count, not one phantom
// extra line from strings.Split's trailing empty field, and must reject an
// offset one past its real end.
func TestReadLineCountIgnoresTrailingNewline(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "f.txt")
	// 3 real lines, newline-terminated (the normal Unix case).
	if err := os.WriteFile(path, []byte("alpha\nbeta\ngamma\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Offset just past the last real line must be rejected with the right total.
	args := mustJSON(ReadArgs{Path: "f.txt", Offset: 4})
	if _, err := readSpec(root, newReadLedger()).Invoke(context.Background(), args); err == nil {
		t.Fatal("offset 4 on a 3-line file should be out of range")
	} else if !strings.Contains(err.Error(), "3 lines total") {
		t.Fatalf("error should report 3 lines total, got: %v", err)
	}

	// Reading the whole file then continuing from the last line must work; the
	// last real line is line 3 and there is nothing after it.
	res, err := readSpec(root, newReadLedger()).Invoke(context.Background(), mustJSON(ReadArgs{Path: "f.txt", Offset: 3}))
	if err != nil {
		t.Fatalf("read offset 3: %v", err)
	}
	if !strings.Contains(res.Content, "gamma") {
		t.Fatalf("offset 3 should show the last line, got: %q", res.Content)
	}
	if strings.Contains(res.Content, "more lines in file") {
		t.Fatalf("offset 3 is the last line; there should be no continuation notice: %q", res.Content)
	}
}

// A truncation notice must report the real total line count, not an inflated one.
func TestReadTruncationTotalIgnoresTrailingNewline(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "big.txt")
	n := DefaultMaxLines + 100
	var b strings.Builder
	for i := 0; i < n; i++ {
		fmt.Fprintf(&b, "line %d\n", i+1)
	}
	if err := os.WriteFile(path, []byte(b.String()), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := readSpec(root, newReadLedger()).Invoke(context.Background(), mustJSON(ReadArgs{Path: "big.txt"}))
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	want := fmt.Sprintf("of %d.", n)
	if !strings.Contains(res.Content, want) {
		t.Fatalf("truncation notice should report %q (real total), got: %q", want, lastLine(res.Content))
	}
}

func lastLine(s string) string {
	parts := strings.Split(strings.TrimRight(s, "\n"), "\n")
	return parts[len(parts)-1]
}
