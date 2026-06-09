// Package coding assembles pi's coding tool runtime: it pairs each spec from
// codingspec with its generated JSON schema and registers them into a
// tool.Registry. It depends on the generated schemas_gen.go, so it is
// intentionally separate from codingspec (which the generator imports) — that
// split is what avoids a generator/consumer bootstrap cycle.
package coding

//go:generate go run github.com/unarbos/arbos/cmd/gen-tool-schemas -specs coding -out schemas_gen.go -pkg coding

import (
	"fmt"

	"github.com/unarbos/arbos/internal/memory"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// NewRuntime builds a tool.Registry with pi's coding tools, their file
// operations scoped to root. It returns an error if a generated schema is
// missing (i.e. someone added a tool but did not run `go generate`).
func NewRuntime(root string, mem *memory.Store) (*tool.Registry, error) {
	r := tool.New()
	for _, s := range codingspec.Specs(root, mem) {
		schema, ok := genSchemas[s.Name]
		if !ok {
			return nil, fmt.Errorf("no generated schema for tool %q; run `go generate ./...`", s.Name)
		}
		if err := r.Register(s, schema); err != nil {
			return nil, err
		}
	}
	return r, nil
}
