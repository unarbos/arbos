package core

import (
	"go/parser"
	"go/token"
	"path/filepath"
	"strings"
	"testing"
)

// TestKernelImportsStaySubstrateFree enforces ADR-0041's load-bearing invariant:
// the cognition kernel (package core) must never import a transport/substrate
// type. Matrix (mautrix/dendrite) and storage-engine packages live behind ports
// and adapters; the day one of them appears in an import here, the hexagonal
// boundary the whole Matrix migration rests on has been breached. This guard
// runs in CI like any other test, so the breach fails the build, not a review.
func TestKernelImportsStaySubstrateFree(t *testing.T) {
	// Substring matches against import paths. Kept deliberately broad: the kernel
	// has no business importing any of these layers.
	forbidden := []string{
		"mautrix",   // Matrix client/appservice/crypto
		"dendrite",  // embedded homeserver
		"gomatrix",  // any Matrix client lib
		"/sqlite",   // the durable store engine (a port, not a kernel dep)
		"/gateway",  // a frontend door
		"/forest",   // the relay/directory
		"/provider", // LLM provider adapters
	}

	fset := token.NewFileSet()
	files, err := filepath.Glob("*.go")
	if err != nil {
		t.Fatalf("glob core sources: %v", err)
	}
	if len(files) == 0 {
		t.Fatal("no core sources found — is the test running in package dir?")
	}
	for _, file := range files {
		if strings.HasSuffix(file, "_test.go") {
			continue // tests may import anything; the guard is on shipped code
		}
		f, err := parser.ParseFile(fset, file, nil, parser.ImportsOnly)
		if err != nil {
			t.Fatalf("parse %s: %v", file, err)
		}
		for _, imp := range f.Imports {
			path := strings.Trim(imp.Path.Value, `"`)
			for _, bad := range forbidden {
				if strings.Contains(path, bad) {
					t.Errorf("%s imports %q — internal/core must stay substrate-free (ADR-0041)", file, path)
				}
			}
		}
	}
}
