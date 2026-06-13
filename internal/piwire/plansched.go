package piwire

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"time"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// wakeTurnTimeout bounds one fired executor turn. A firing is one unit of
// work (a recurrence, a deferred task), not a mission; anything bigger should
// decompose into the plan and finish across future turns or firings.
// ARBOS_WAKE_TIMEOUT (whole seconds) overrides it for missions whose single
// firing legitimately needs longer — the retry/fallback chain inside a turn
// must fit under this bound, so a host that fronts a flaky provider may want
// more headroom.
const wakeTurnTimeout = 15 * time.Minute

// wakeTimeout resolves the per-firing turn bound from the environment, falling
// back to wakeTurnTimeout. A non-positive or unparseable value keeps the
// default rather than disabling the bound (an unbounded wake could wedge a
// concurrency slot forever).
func wakeTimeout() time.Duration {
	if sec, err := strconv.Atoi(os.Getenv("ARBOS_WAKE_TIMEOUT")); err == nil && sec > 0 {
		return time.Duration(sec) * time.Second
	}
	return wakeTurnTimeout
}

// StartPlanScheduler attaches the host's clock: the plan scheduler scanning
// for time-armed nodes (internal/plan) with a wake that spawns a fresh
// executor session per firing. Fresh on purpose — context is a consumable,
// and the forest injection re-grounds the executor better than any warmed-
// over history would. It returns a stop function, or nil when the store
// cannot back a plan (the in-memory fake), so callers can wire it
// unconditionally. Only the long-running serve host should call it: the
// scheduler is the part of the agent that owns a watch, and a one-shot run
// does not.
func (h *Host) StartPlanScheduler() func() {
	ps, ok := h.store.(plan.Store)
	if !ok {
		return nil
	}
	cwd, _ := os.Getwd()
	// Under -approve the user has asked to gate every workspace mutation, so
	// the kernel must not auto-run shell: with a nil runner, cmd nodes stay
	// [ready] and are executed only inside a turn, through the approval-gated
	// bash tool. Timed and callback wakes still fire — they spawn a session
	// that is itself gated. The notify executor is not a workspace mutation,
	// so it runs regardless of approval.
	var run plan.CmdRunner
	if !h.approve {
		run = cmdRunner(cwd)
	}
	var notify plan.NotifyFunc
	if nf, ok := h.store.(interface {
		NotifyFrom(ctx context.Context, text, session, source string) error
	}); ok {
		// The node id is the coalescing source: a recurring ping's new firing
		// replaces its own undelivered predecessor, so a closed door gets the
		// latest reading on reopen, never the backlog.
		notify = func(ctx context.Context, msg, session string, node plan.NodeID) error {
			return nf.NotifyFrom(ctx, msg, session, core.SpawnedByNode(int64(node)))
		}
	} else if ob, ok := h.store.(outbox.Store); ok {
		notify = func(ctx context.Context, msg, session string, _ plan.NodeID) error {
			return ob.Notify(ctx, msg, session)
		}
	}
	// The schedule axis (a wake) spawns through the same Agent mechanism as
	// the call axis (delegate): "self" is the full controller engine wrapped
	// as an Agent, so a firing is just self.Run — no hand-rolled session
	// management. delegate passes a relay sink and joins the result up the
	// stack; the scheduler passes nil and lets the work land in durable state.
	// One spawn, two callers.
	self := agent.NewArbosAgent(
		func(agent.Grant) (*engine.Engine, error) { return h.Engine, nil },
		NewSessionID,
	)
	// Couple the orphan-reclaim window to the (possibly raised) wake timeout so
	// it always exceeds the longest a live wake can run by a wide margin — a
	// reclaim that fired on a still-running wake would reset its node out from
	// under it. wakeReclaimMargin is the headroom above the timeout.
	sched := plan.NewScheduler(ps, sysClock{}, selfWake(self), run, notify, h.logger,
		plan.WithReclaimAfter(wakeTimeout()+wakeReclaimMargin))
	return sched.Close
}

// wakeReclaimMargin is how far the orphan-reclaim window sits above the wake
// turn timeout, so a slow-but-live wake is never reclaimed mid-run.
const wakeReclaimMargin = 45 * time.Minute

// cmdJobTimeout bounds one kernel-run command node. Mechanical pipeline steps
// (builds, tests, pushes) finish in minutes; anything longer should be a
// background job the model supervises.
const cmdJobTimeout = 30 * time.Minute

// cmdRunner executes a node's command through the workspace job supervisor,
// so kernel runs are first-class jobs: journaled, killable, and visible in
// the same job table the model and the front door read.
func cmdRunner(cwd string) plan.CmdRunner {
	return func(ctx context.Context, command string) (string, int, string, error) {
		ctx, cancel := context.WithTimeout(ctx, cmdJobTimeout)
		defer cancel()
		return codingspec.RunWorkspaceCmd(ctx, cwd, command)
	}
}

// selfWake runs one fired node as one fresh controller session, through the
// Agent spawn mechanism (self.Run) rather than hand-rolled session management:
// the prompt says which node's moment arrived, and the agent acts on the
// forest its context already carries. emit is nil — a wake is the schedule
// axis, depth 0, with no caller to relay to or return a result up; the work
// lands in durable state. Task.Origin stamps the session OriginScheduler at
// creation (not after), so the front-door brief anchors "since you left" on
// human-driven sessions only, with no window or silent-failure path.
func selfWake(self agent.Agent) plan.WakeFunc {
	timeout := wakeTimeout()
	return func(ctx context.Context, w plan.WakeEvent) error {
		ctx, cancel := context.WithTimeout(ctx, timeout)
		defer cancel()
		// Owner ties the wake to the conversation whose node fired: its
		// notifications route back there (sqlite.Store.Notify resolves the
		// owner), and the gateway lists the chat's runs so its UI can open
		// the wake's transcript as a tab.
		_, err := self.Run(ctx, agent.Task{
			Instruction: wakePrompt(w),
			Origin:      core.OriginScheduler,
			Owner:       core.SessionID(w.Node.Origin),
			SpawnedBy:   core.SpawnedByNode(int64(w.Node.ID)),
		}, nil)
		return err
	}
}

func wakePrompt(w plan.WakeEvent) string {
	n := w.Node
	switch w.Reason {
	case plan.WakeCmdFailed:
		detail := w.Detail
		if detail == "" {
			detail = "(no output captured)"
		}
		return fmt.Sprintf(
			"Kernel-run command failed: node #%d — %s. Command: `%s`. Output tail:\n%s\nDiagnose and act: fix the cause and reopen the node (status pending) so the kernel retries, adjust its cmd, or record it failed/blocked with an outcome. No conversation is open: anything the user must hear goes through the notify tool — and only what genuinely concerns them.",
			n.ID, n.Goal, n.Cmd, detail)
	case plan.WakeReady:
		return fmt.Sprintf(
			"Callback firing: node #%d is now ready — %s. Its earlier siblings just finished (see the <<plan>> block for their outcomes). Claim it (op:update, node %d, status active), do what it says — usually reporting completion to the user via the notify tool — and record it done with an outcome. No conversation is open: notify is your only voice.",
			n.ID, n.Goal, n.ID)
	case plan.WakeDue:
		if n.Recurring() {
			return fmt.Sprintf(
				"Scheduled firing: standing obligation #%d is due — %s. Do it now, then record the recurrence with the plan tool (op:update, node %d, outcome only). Keep it to this one obligation. No conversation is open: anything the user must hear goes through the notify tool.",
				n.ID, n.Goal, n.ID)
		}
		return fmt.Sprintf(
			"Scheduled firing: deferred task #%d is now due — %s. Claim it with the plan tool (op:update, node %d, status active), do it, and record the result (done or failed, with an outcome). No conversation is open: anything the user must hear goes through the notify tool.",
			n.ID, n.Goal, n.ID)
	case plan.WakeCondition:
		detail := w.Detail
		if detail == "" {
			detail = "(predicate produced no output)"
		}
		return fmt.Sprintf(
			"Condition met: the watch on node #%d held — %s. Predicate: `%s`. Its latest output:\n%s\nThe kernel keeps polling this node, so do NOT manage its status; just act on the goal now. No conversation is open: anything the user must hear goes through the notify tool.",
			n.ID, n.Goal, n.Cond, detail)
	}
	// WakeReason is a closed enum; a new value must add a branch above rather
	// than silently inherit a prompt.
	return fmt.Sprintf("Node #%d (%s) needs attention. Inspect the plan and act.", n.ID, n.Goal)
}
