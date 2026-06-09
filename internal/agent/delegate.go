package agent

import (
	"context"
	"fmt"
	"reflect"

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
}

// RegisterDelegate adds the delegate tool to a registry, routing through r.
//
// Note: as a ToolRuntime tool, delegate runs the child to completion and returns
// its final text as the tool result; it does not stream the child's events into
// the parent's live stream (that would need engine-level streaming-tool support).
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
	spec := tool.NewSpec("delegate", "Delegate a sub-task to another agent and return its result.", false,
		func(ctx context.Context, a DelegateArgs) (string, error) {
			ag, err := r.resolve(BackendRef(a.Backend))
			if err != nil {
				return "", err
			}
			res, err := ag.Run(ctx, Task{
				Instruction: a.Instruction,
				Backend:     BackendRef(a.Backend),
				Grant:       Grant{Tools: a.Tools},
			}, nil)
			if err != nil {
				return "", err
			}
			return res.Text, nil
		})
	return reg.Register(spec, schema)
}

// StartCodingSessionArgs are the arguments to the start_coding_session sugar
// tool.
type StartCodingSessionArgs struct {
	Goal string `json:"goal" desc:"The coding goal for the session."`
	Repo string `json:"repo,omitempty" desc:"Working directory / repo the session runs in (default: current)."`
}

// RegisterStartCodingSession adds the start_coding_session sugar tool, which
// desugars to a delegation to the "pi" coding backend (the tool named in pi's
// delegation docs). It is the one-call entry to spin up a coding agent; it
// routes through the same Router as delegate, so there is one dispatch path.
func RegisterStartCodingSession(reg *tool.Registry, r *Router) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(StartCodingSessionArgs{}))
	if err != nil {
		return fmt.Errorf("start_coding_session schema: %w", err)
	}
	spec := tool.NewSpec("start_coding_session", "Start a coding session: delegate a coding goal to the pi coding agent and return its result.", false,
		func(ctx context.Context, a StartCodingSessionArgs) (string, error) {
			ag, err := r.resolve("pi")
			if err != nil {
				return "", err
			}
			res, err := ag.Run(ctx, Task{
				Instruction: a.Goal,
				Backend:     "pi",
				Grant:       Grant{Env: EnvironmentRef{Path: a.Repo}},
			}, nil)
			if err != nil {
				return "", err
			}
			return res.Text, nil
		})
	return reg.Register(spec, schema)
}
