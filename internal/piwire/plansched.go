package piwire

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
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
	sched := plan.NewScheduler(ps, sysClock{}, planWake(h.Engine, h.store), cmdRunner(cwd), h.logger)
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

// planWake runs one fired node as one fresh session turn: prompt it with the
// firing, drain its events, and let the session end with the turn. The
// executor sees the whole forest via the plan injection; the prompt only has
// to say which node's moment arrived and what closing the loop looks like.
// The session is stamped OriginScheduler so the front-door brief can tell
// machine-initiated work from sessions a human drove.
func planWake(eng *engine.Engine, store ports.SessionStore) plan.WakeFunc {
	return func(ctx context.Context, w plan.WakeEvent) error {
		ctx, cancel := context.WithTimeout(ctx, wakeTurnTimeout)
		defer cancel()
		conv, err := eng.StartSession(ctx, NewSessionID())
		if err != nil {
			return fmt.Errorf("plan wake: start session: %w", err)
		}
		if sess, err := store.Get(ctx, conv.ID()); err == nil {
			sess.Origin = core.OriginScheduler
			_ = store.UpdateSession(ctx, sess) // best-effort: only the brief's anchor depends on it
		}
		conv.Send(core.PromptIntent{Text: wakePrompt(w)})
		for {
			select {
			case <-ctx.Done():
				return ctx.Err()
			case env, ok := <-conv.Events():
				if !ok {
					return nil
				}
				switch ev := env.Event.(type) {
				case core.TurnComplete:
					return nil
				case core.ErrorEvent:
					if env.SessionID == conv.ID() && !ev.Retryable {
						return fmt.Errorf("plan wake: %s", ev.Err)
					}
				}
			}
		}
	}
}

func wakePrompt(w plan.WakeEvent) string {
	n := w.Node
	if w.Reason == plan.WakeCmdFailed {
		detail := w.Detail
		if detail == "" {
			detail = "(no output captured)"
		}
		return fmt.Sprintf(
			"Kernel-run command failed: node #%d — %s. Command: `%s`. Output tail:\n%s\nDiagnose and act: fix the cause and reopen the node (status pending) so the kernel retries, adjust its cmd, or record it failed/blocked with an outcome. No conversation is open: anything the user must hear goes through the notify tool — and only what genuinely concerns them.",
			n.ID, n.Goal, n.Cmd, detail)
	}
	if w.Reason == plan.WakeReady {
		return fmt.Sprintf(
			"Callback firing: node #%d is now ready — %s. Its earlier siblings just finished (see the <<plan>> block for their outcomes). Claim it (op:update, node %d, status active), do what it says — usually reporting completion to the user via the notify tool — and record it done with an outcome. No conversation is open: notify is your only voice.",
			n.ID, n.Goal, n.ID)
	}
	if n.Kind == plan.KindMaintain {
		return fmt.Sprintf(
			"Scheduled firing: standing obligation #%d is due — %s. Do it now, then record the recurrence with the plan tool (op:update, node %d, outcome only). Keep it to this one obligation. No conversation is open: anything the user must hear goes through the notify tool.",
			n.ID, n.Goal, n.ID)
	}
	return fmt.Sprintf(
		"Scheduled firing: deferred task #%d is now due — %s. Claim it with the plan tool (op:update, node %d, status active), do it, and record the result (done or failed, with an outcome). No conversation is open: anything the user must hear goes through the notify tool.",
		n.ID, n.Goal, n.ID)
}
