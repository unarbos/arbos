package codingspec

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/unarbos/arbos/internal/tool"
)

func TestWriteStalenessUsesCanonicalResourceKey(t *testing.T) {
	root := t.TempDir()
	realDir := filepath.Join(root, "real")
	if err := os.Mkdir(realDir, 0o755); err != nil {
		t.Fatal(err)
	}
	linkDir := filepath.Join(root, "link")
	if err := os.Symlink(realDir, linkDir); err != nil {
		t.Skipf("symlinks unsupported: %v", err)
	}
	realPath := filepath.Join(realDir, "file.txt")
	if err := os.WriteFile(realPath, []byte("base"), 0o644); err != nil {
		t.Fatal(err)
	}

	ledger := newReadLedger()
	cp := newCheckpointer(root)
	read := readSpec(root, ledger)
	write := writeSpec(root, ledger, cp)

	readArgs, _ := json.Marshal(ReadArgs{Path: filepath.Join("link", "file.txt")})
	if _, err := read.Invoke(context.Background(), readArgs); err != nil {
		t.Fatalf("read via symlink: %v", err)
	}
	if err := os.WriteFile(realPath, []byte("other actor"), 0o644); err != nil {
		t.Fatal(err)
	}

	writeArgs, _ := json.Marshal(WriteArgs{Path: filepath.Join("real", "file.txt"), Content: "ours"})
	_, err := write.Invoke(context.Background(), writeArgs)
	if err == nil {
		t.Fatal("write through canonical path succeeded after symlink read went stale")
	}
	if !strings.Contains(err.Error(), "changed on disk since you last saw it") {
		t.Fatalf("unexpected stale error: %v", err)
	}
	got, err := os.ReadFile(realPath)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "other actor" {
		t.Fatalf("stale write clobbered file: got %q", got)
	}
}

func TestConcurrentWritesToSameSeenFileDoNotSilentlyClobber(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "file.txt")
	if err := os.WriteFile(path, []byte("base"), 0o644); err != nil {
		t.Fatal(err)
	}

	cp := newCheckpointer(root)
	ledgerA := newReadLedger()
	ledgerB := newReadLedger()
	readA := readSpec(root, ledgerA)
	readB := readSpec(root, ledgerB)
	writeA := writeSpec(root, ledgerA, cp)
	writeB := writeSpec(root, ledgerB, cp)
	ctx := context.Background()

	readArgs, _ := json.Marshal(ReadArgs{Path: "file.txt"})
	if _, err := readA.Invoke(ctx, readArgs); err != nil {
		t.Fatalf("read A: %v", err)
	}
	if _, err := readB.Invoke(ctx, readArgs); err != nil {
		t.Fatalf("read B: %v", err)
	}

	var wg sync.WaitGroup
	errs := make(chan error, 2)
	writes := []struct {
		content string
		invoke  func(context.Context, json.RawMessage) (tool.Result, error)
	}{
		{content: "first", invoke: writeA.Invoke},
		{content: "second", invoke: writeB.Invoke},
	}
	for _, w := range writes {
		wg.Add(1)
		go func(content string, invoke func(context.Context, json.RawMessage) (tool.Result, error)) {
			defer wg.Done()
			args, _ := json.Marshal(WriteArgs{Path: "file.txt", Content: content})
			_, err := invoke(ctx, args)
			errs <- err
		}(w.content, w.invoke)
	}
	wg.Wait()
	close(errs)

	successes := 0
	stales := 0
	for err := range errs {
		switch {
		case err == nil:
			successes++
		case strings.Contains(err.Error(), "changed on disk since you last saw it"):
			stales++
		default:
			t.Fatalf("unexpected write error: %v", err)
		}
	}
	if successes != 1 || stales != 1 {
		t.Fatalf("want one success and one stale failure, got successes=%d stales=%d", successes, stales)
	}
}
