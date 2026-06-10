package piwire

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/unarbos/arbos/internal/agent"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// wakeTurnTimeout bounds one fired executor turn. A firing is one unit of
// work (a recurrence, a deferred task), not a mission; anything bigger should
// decompose into the plan and finish across future turns or firings.
const wakeTurnTimeout = 15 * time.Minute

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
	if ob, ok := h.store.(outbox.Store); ok {
		notify = ob.Notify
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
	sched := plan.NewScheduler(ps, sysClock{}, selfWake(self, h.store), run, notify, h.logger)
	return sched.Close
}

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
// lands in durable state. The spawned session is stamped OriginScheduler so
// the front-door brief anchors "since you left" on human-driven sessions only.
func selfWake(self agent.Agent, store ports.SessionStore) plan.WakeFunc {
	return func(ctx context.Context, w plan.WakeEvent) error {
		ctx, cancel := context.WithTimeout(ctx, wakeTurnTimeout)
		defer cancel()
		res, err := self.Run(ctx, agent.Task{Instruction: wakePrompt(w)}, nil)
		if res.ChildSession != "" {
			// Detached stamp: the session's events are written; LastHumanSeen
			// filters on the session row's origin, so stamping after the run
			// (with a fresh context) still anchors the brief correctly.
			if sess, gerr := store.Get(context.Background(), core.SessionID(res.ChildSession)); gerr == nil {
				sess.Origin = core.OriginScheduler
				_ = store.UpdateSession(context.Background(), sess)
			}
		}
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
	}
	// WakeReason is a closed enum; a new value must add a branch above rather
	// than silently inherit a prompt.
	return fmt.Sprintf("Node #%d (%s) needs attention. Inspect the plan and act.", n.ID, n.Goal)
}
