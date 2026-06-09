// Package codingspec declares pi's coding toolset: argument structs, handlers,
// and metadata, scoped to a workspace root. It compiles standalone (no
// dependency on the generated schemas) so the schema generator can reflect the
// arg types without a bootstrap cycle. The assembler in package coding pairs
// each spec here with its generated schema.
//
// This is the Go port of pi's coding-agent tools. It lands one tool per phase:
// ls, read, find (read-only navigation) so far; grep, write, edit, and bash
// follow. Each handler reproduces pi's model-facing output faithfully — the
// result text is itself part of pi's intelligence.
package codingspec

import (
	"github.com/unarbos/arbos/internal/memory"
	"github.com/unarbos/arbos/internal/tool"
)

// Specs returns the coding tool specs with handlers bound to root. mem may be nil
// to omit the memory tool. The generator passes "" and nil because it only
// reflects ArgSample types; the assembler passes the real workspace root and store.
func Specs(root string, mem *memory.Store) []tool.Spec {
	return []tool.Spec{
		lsSpec(root),
		readSpec(root),
		findSpec(root),
		grepSpec(root),
		writeSpec(root),
		editSpec(root),
		bashSpec(root),
		fetchSpec(),
		memorySpec(mem),
	}
}
