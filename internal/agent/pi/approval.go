package pi

import (
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// CodingApprovalPolicy gates the mutating coding tools behind human
// confirmation. It is OFF by default: pi runs with full privileges unless a
// user opts in by setting Options.Approval to this policy (e.g. `arbos -approve`
// or ARBOS_APPROVE=1 for the TUI). pi itself has no permission system; this is
// arbos giving the user a safety valve pi lacks, without changing the default.
type CodingApprovalPolicy struct {
	// Gated is the set of tool names that require approval. Nil uses the default
	// mutating set (write, edit, bash); read-only tools never require approval.
	Gated map[string]bool
}

var _ ports.ApprovalPolicy = CodingApprovalPolicy{}

var defaultGatedTools = map[string]bool{"write": true, "edit": true, "bash": true}

// Requires gates the mutating tools and approves everything else (read-only
// navigation and search run unattended).
func (p CodingApprovalPolicy) Requires(call core.ToolCall) (string, bool) {
	gated := p.Gated
	if gated == nil {
		gated = defaultGatedTools
	}
	if gated[call.Name] {
		return "the " + call.Name + " tool mutates the workspace", true
	}
	return "", false
}
