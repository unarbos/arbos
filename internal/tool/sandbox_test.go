package tool

import (
	"os"
	"path/filepath"
	"testing"
)

// Resolve is unrestricted by default (arbos trusts its human-monitored agents):
// these pin that a path may point outside base. ResolveWithin is the opt-in
// jail; its tests assert the boundary the default deliberately drops.

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
		t.Fatalf("in-root path should resolve: %v", err)
	}
	if want := filepath.Join(root, "notes/hello.txt"); got != want {
		t.Fatalf("resolve mismatch: got %q want %q", got, want)
	}
}

func TestResolveWithin_RefusesLexicalEscape(t *testing.T) {
	root := t.TempDir()
	if _, err := ResolveWithin(root, "../../../../etc/passwd"); err == nil {
		t.Fatal("lexical traversal out of root must be refused in confined mode")
	}
}

func TestResolveWithin_RefusesAbsoluteEscape(t *testing.T) {
	root := t.TempDir()
	if _, err := ResolveWithin(root, "/etc/passwd"); err == nil {
		t.Fatal("absolute path outside root must be refused in confined mode")
	}
}

func TestResolveWithin_RefusesSymlinkEscape(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir()
	if err := os.WriteFile(filepath.Join(outside, "secret.txt"), []byte("top secret"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "escape")); err != nil {
		t.Skipf("symlinks unsupported: %v", err)
	}
	if _, err := ResolveWithin(root, "escape/secret.txt"); err == nil {
		t.Fatal("symlink escape out of root must be refused")
	}
}

func TestResolveWithin_AllowsWithinRoot(t *testing.T) {
	root := t.TempDir()
	got, err := ResolveWithin(root, "notes/hello.txt")
	if err != nil {
		t.Fatalf("in-root path should resolve: %v", err)
	}
	if want := filepath.Join(root, "notes/hello.txt"); got != want {
		t.Fatalf("resolve mismatch: got %q want %q", got, want)
	}
}
