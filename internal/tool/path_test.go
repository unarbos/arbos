package tool

import (
	"os"
	"path/filepath"
	"testing"
)

// These pin Resolve's contract: paths resolve, they are never confined. The
// working directory is a starting point, not a boundary.

func TestResolve_PermitsRelativeTraversal(t *testing.T) {
	root := t.TempDir()
	// "../" reaches a sibling of root, like a shell — no clamp, no error.
	got, err := Resolve(root, "../sibling.txt")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if want := filepath.Join(filepath.Dir(root), "sibling.txt"); got != want {
		t.Fatalf("traversal mismatch: got %q want %q", got, want)
	}
}

func TestResolve_PermitsAbsolutePath(t *testing.T) {
	root := t.TempDir()
	got, err := Resolve(root, "/etc/passwd")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got != "/etc/passwd" {
		t.Fatalf("absolute path mismatch: got %q want %q", got, "/etc/passwd")
	}
}

func TestResolve_JoinsRelativeToBase(t *testing.T) {
	root := t.TempDir()
	got, err := Resolve(root, "notes/hello.txt")
	if err != nil {
		t.Fatalf("in-base path should resolve: %v", err)
	}
	if want := filepath.Join(root, "notes/hello.txt"); got != want {
		t.Fatalf("resolve mismatch: got %q want %q", got, want)
	}
}

func TestResourceKey_CanonicalizesSymlinkAncestor(t *testing.T) {
	root := t.TempDir()
	realDir := filepath.Join(root, "real")
	if err := os.Mkdir(realDir, 0o755); err != nil {
		t.Fatal(err)
	}
	linkDir := filepath.Join(root, "link")
	if err := os.Symlink(realDir, linkDir); err != nil {
		t.Skipf("symlinks unsupported: %v", err)
	}

	realPath := filepath.Join(realDir, "new.txt")
	linkPath := filepath.Join(linkDir, "new.txt")
	if got, want := ResourceKey(linkPath), ResourceKey(realPath); got != want {
		t.Fatalf("resource key mismatch: got %q want %q", got, want)
	}
}
