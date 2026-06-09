package memory_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/unarbos/arbos/internal/memory"
)

func TestStoreRememberRecall(t *testing.T) {
	dir := t.TempDir()
	s := memory.NewStore("", dir)
	if err := s.Remember("color", "ultramarine"); err != nil {
		t.Fatal(err)
	}
	v, ok, err := s.Recall("color")
	if err != nil || !ok || v != "ultramarine" {
		t.Fatalf("recall = %q ok=%v err=%v", v, ok, err)
	}
	text, err := s.FormatMerged()
	if err != nil || text == "" {
		t.Fatalf("format: %q err=%v", text, err)
	}
}

func TestStoreProjectOverridesUser(t *testing.T) {
	user := filepath.Join(t.TempDir(), "user")
	proj := t.TempDir()
	userMem := filepath.Join(user, "memory")
	if err := os.MkdirAll(userMem, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(userMem, "memories.json"), []byte(`{"entries":{"k":"user-val"}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	s := memory.NewStore(user, proj)
	if err := s.Remember("k", "project-val"); err != nil {
		t.Fatal(err)
	}
	v, ok, err := s.Recall("k")
	if err != nil || !ok || v != "project-val" {
		t.Fatalf("project should win: %q ok=%v err=%v", v, ok, err)
	}
}
