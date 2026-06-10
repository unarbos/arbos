// Package codingspec declares pi's coding toolset: argument structs, handlers,
// and metadata, rooted at a working directory (a starting point, not a
// boundary — paths resolve anywhere, see tool.Resolve). It compiles standalone
// (no dependency on the generated schemas) so the schema generator can reflect
// the arg types without a bootstrap cycle. The assembler in package coding
// pairs each spec here with its generated schema.
//
// This is the Go port of pi's coding-agent tools.
// Each handler reproduces pi's model-facing output faithfully — the
// result text is itself part of pi's intelligence. Deliberate divergences are
// documented where they live: edit's matcher, errors, and result snippet
// (editdiff.go, edit.go) and bash's background jobs (job.go) extend pi rather
// than port it.
package codingspec

import (
	"github.com/unarbos/arbos/internal/tool"
)

// Specs returns the coding tool specs with handlers bound to root (the session's
// working directory). The generator passes "" because it only reflects ArgSample
// types; the assembler passes the real cwd.
func Specs(root string) []tool.Spec {
	// One ledger per toolset: the top-level session and each delegated child
	// assemble their own Specs, so none shares another's read history. The
	// ledger's memory is scoped to the current turn (see readLedger.reconcile).
	ledger := newReadLedger()
	// One job supervisor per toolset, but the job table itself is the shared
	// per-workspace directory on disk (see job.go), so a delegated child or a
	// restarted arbos sees the same jobs.
	jobs := newJobSupervisor(root)
	// One checkpointer per toolset: it snapshots the workspace to a git ref
	// before the first mutation of each turn, so undo has a restore point.
	cp := newCheckpointer(root)
	return []tool.Spec{
		lsSpec(root),
		readSpec(root, ledger),
		findSpec(root),
		grepSpec(root),
		writeSpec(root, ledger, cp),
		editSpec(root, ledger, cp),
		bashSpec(root, jobs, cp),
		awaitSpec(jobs),
		jobsSpec(jobs),
		fetchSpec(),
		changesSpec(cp),
		undoSpec(cp),
	}
}
