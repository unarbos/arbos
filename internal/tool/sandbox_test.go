package tool

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// These assert the security-critical sandbox directly on Resolve (the single
// home all file tools route through). They previously lived against the
// now-removed builtin toolset; the guarantees are the same.

func TestResolve_ClampsLexicalTraversal(t *testing.T) {
	root := t.TempDir()
	// Leading "../" is neutralized by clamping into root rather than erroring;
	// the security property is that the result never escapes root.
	got, err := Resolve(root, "../../../../etc/passwd")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.HasPrefix(got, root) {
		t.Fatalf("traversal escaped root: %q is not under %q", got, root)
	}
}

func TestResolve_RefusesSymlinkEscape(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir()
	if err := os.WriteFile(filepath.Join(outside, "secret.txt"), []byte("top secret"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "escape")); err != nil {
		t.Skipf("symlinks unsupported: %v", err)
	}
	if _, err := Resolve(root, "escape/secret.txt"); err == nil {
		t.Fatal("symlink escape out of root must be refused")
	}
}

func TestResolve_AllowsWithinRoot(t *testing.T) {
	root := t.TempDir()
	got, err := Resolve(root, "notes/hello.txt")
	if err != nil {
		t.Fatalf("in-root path should resolve: %v", err)
	}
	if want := filepath.Join(root, "notes/hello.txt"); got != want {
		t.Fatalf("resolve mismatch: got %q want %q", got, want)
	}
}
