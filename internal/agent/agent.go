// Package agent is the composition layer above the leaf ports: an Agent runs a
// Task and streams KernelEvents, and delegation is one agent invoking another as
// a child parameterized by a Grant (tools + environment + budget). A plain LLM
// query is the degenerate Agent. The parent never cares what kind of agent the
// child is — it hands off a Task and consumes events — which is what makes
// "agents all the way down" cohere (ADR-0013).
//
// This package ships the local ArbosAgent (a nested kernel session). pi is
// registered as a delegatable backend (internal/agent/pi), so a running session
// spawns coding sub-agents through this same interface.
package agent

import (
	"context"

	"github.com/unarbos/arbos/internal/core"
)

// Agent runs a Task to completion, streaming KernelEvents through emit (which
// may be nil if the caller does not want live events) and returning a Result.
type Agent interface {
	Run(ctx context.Context, t Task, emit func(core.KernelEvent)) (Result, error)
}

// Task is a unit of delegated work.
type Task struct {
	Instruction string     // the prompt / goal
	Backend     BackendRef // which agent runtime should run it
	Grant       Grant      // what the child may use
}

// BackendRef names a backend (e.g. "pi").
type BackendRef string

// Grant is the sharing primitive: it bounds what a delegated child may use.
type Grant struct {
	Tools  []string       // tool/toolset allowlist for the child
	Env    EnvironmentRef // where it runs
	Budget Budget         // bounds the whole subtree
}

// EnvironmentRef points at where a child runs. Path roots the child's workspace
// (its tool sandbox and prompt cwd); empty means inherit the parent's cwd.
type EnvironmentRef struct {
	Path string
}

// Budget bounds a delegated subtree. Zero fields mean "inherit a sensible
// default" at the factory.
type Budget struct {
	MaxIterations int
}

// Result is what a delegated agent returns to its parent.
type Result struct {
	Text         string
	Usage        core.Usage
	ChildSession string
}
