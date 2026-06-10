package agent

import (
	"context"
	"fmt"
	"reflect"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// Router resolves a BackendRef to the Agent that runs it. It is the single
// dispatch point the delegate tool (and its sugar tools) route through, so there
// is one delegation path rather than one per backend.
type Router struct {
	agents  map[BackendRef]Agent
	defName BackendRef
}

func NewRouter() *Router { return &Router{agents: make(map[BackendRef]Agent)} }

// Register adds an agent under a backend name. The first registered backend
// becomes the default used when a delegate call omits one.
func (r *Router) Register(ref BackendRef, ag Agent) {
	if len(r.agents) == 0 {
		r.defName = ref
	}
	r.agents[ref] = ag
}

func (r *Router) resolve(ref BackendRef) (Agent, error) {
	if ref == "" {
		ref = r.defName
	}
	ag, ok := r.agents[ref]
	if !ok {
		return nil, fmt.Errorf("unknown backend %q", ref)
	}
	return ag, nil
}

// DelegateArgs are the arguments to the delegate tool.
type DelegateArgs struct {
	Instruction string   `json:"instruction" desc:"What the delegated agent should do."`
	Backend     string   `json:"backend,omitempty" desc:"Which backend runs it; defaults to the primary."`
	Tools       []string `json:"tools,omitempty" desc:"Tool/toolset allowlist granted to the child."`
	Cwd         string   `json:"cwd,omitempty" desc:"Working directory / repo the child runs in (default: inherit the parent's cwd)."`
}

// RegisterDelegate adds the delegate tool to a registry, routing through r.
//
// The child's events stream into the parent's live stream via the relay sink the
// engine attaches to the dispatch context (engine.Relay), so a delegated turn is
// no longer opaque until completion — its activity renders nested under the
// parent in real time, and the final text still returns as the tool result.
//
// Every delegation advertises an empty footprint, so siblings fan out in
// parallel regardless of whether they read or write — N coding sub-agents run at
// once, the way Cursor runs parallel agents. arbos does not serialize writers or
// isolate them in worktrees by default: it assumes a human is watching and git
// is the net. Confining writers to copy-on-write worktrees is the opt-in safe
// mode, not the baseline.
func RegisterDelegate(reg *tool.Registry, r *Router) error {
	// Intentional carve-out from ADR-0004's "no runtime reflection": the codegen
	// path covers the STATIC built-in catalog (compiled into the binary, drift-
	// checked in CI). delegate is registered dynamically by whoever wires a
	// Router, so there is no build-time site to generate from — reflecting its
	// arg struct once at registration is the analog, and jsonschema is the same
	// reflector the generator uses, so the schema shape stays identical.
	schema, err := jsonschema.Reflect(reflect.TypeOf(DelegateArgs{}))
	if err != nil {
		return fmt.Errorf("delegate schema: %w", err)
	}
	spec := tool.NewSpec("delegate", "Delegate a sub-task to another agent and return its result. Issue several delegate calls at once and they run in parallel — the way to explore or edit a large codebase with multiple sub-agents working concurrently.", false,
		func(ctx context.Context, a DelegateArgs) (string, error) {
			ag, err := r.resolve(BackendRef(a.Backend))
			if err != nil {
				return "", err
			}
			res, err := ag.Run(ctx, Task{
				Instruction: a.Instruction,
				Backend:     BackendRef(a.Backend),
				Grant:       Grant{Tools: a.Tools, Env: EnvironmentRef{Path: a.Cwd}},
			}, engine.Relay(ctx))
			if err != nil {
				return "", err
			}
			return res.Text, nil
		})
	// A delegation's footprint is always empty: siblings never conflict, so the
	// engine fans them out concurrently. Isolation between parallel writers is a
	// host concern (opt-in worktrees), not something to buy by serializing here.
	spec = tool.WithAccess(spec, func(DelegateArgs) core.AccessSet {
		return core.AccessSet{}
	})
	return reg.Register(spec, schema)
}
