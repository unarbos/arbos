package plan

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"sort"
	"time"

	"github.com/unarbos/arbos/internal/ports"
)

// The scheduler is the host's clock: the one place where time initiates work
// instead of responding to it. It scans the forest for time-armed nodes whose
// moment has arrived — maintain nodes past NextDue, deferred achieve nodes
// past After — disarms each one, and wakes an executor for it. Everything
// mechanical lives here, deterministically (the kernel side of the
// kernel/judgment split); what to do upon waking is the model's, in the
// session the wake spawns.
//
// It deliberately fires only TIME-ARMED nodes. Ordinary ready nodes are pull
// mode's job (the projection marks them and a running turn picks them up);
// driving a whole plan unprompted is the future driver's job, opt-in. The
// scheduler is just the part of the agent that owns a watch.
//
// Durability comes from the store, not the process: NextDue and After are
// rows, so a restarted host resumes exactly where the dead one stood, and a
// firing missed while down coalesces into one wake on the next scan (the
// re-arm is measured from now, never replayed per missed period).

// WakeReason says why the model is being summoned.
type WakeReason string

const (
	// WakeDue: a time-armed judgment node's moment arrived.
	WakeDue WakeReason = "due"
	// WakeCmdFailed: a kernel-run command exited non-zero — the exception
	// handler case. The model appears precisely at failures, nowhere else.
	WakeCmdFailed WakeReason = "cmd_failed"
)

// WakeEvent is one summons: the node, why, and any mechanical context (the
// failing command's output tail).
type WakeEvent struct {
	Node   Node
	Reason WakeReason
	Detail string
}

// WakeFunc starts an executor for a firing. The host supplies it (a fresh
// engine session prompted with the event); the scheduler stays free of any
// engine dependency. It is called on the scheduler's drain goroutine and may
// run for the length of a whole turn.
type WakeFunc func(ctx context.Context, w WakeEvent) error

// CmdRunner executes a node's command in the workspace and waits for it,
// returning the job id, exit code, and a short output tail. The host supplies
// it (the job supervisor, so kernel runs are visible in the job table like
// any other job); the scheduler stays free of process machinery.
type CmdRunner func(ctx context.Context, command string) (jobID string, exitCode int, tail string, err error)

// scanInterval is how often the scheduler checks for due nodes. Recurrence
// periods are minutes-to-hours; a coarse scan keeps the idle host at one cheap
// query per tick instead of a precise per-node timer heap it doesn't need yet.
const scanInterval = 30 * time.Second

// Scheduler owns the scan loop. One per host process (the concurrency
// corollary: a single goroutine owns firing, so disarm-then-wake can never
// race itself; cross-process double-fire is out of scope until the self is
// shared between hosts).
type Scheduler struct {
	store  Store
	clock  ports.Clock
	wake   WakeFunc
	run    CmdRunner
	log    *slog.Logger
	ticker *time.Ticker
	// kick wakes the drain loop ahead of the next tick — a finished command
	// may have just ungated its successor, and a four-step pipeline should
	// flow at execution speed, not at 30-second tick quanta.
	kick chan struct{}
	stop chan struct{}
	done chan struct{}
}

// NewScheduler starts the scan loop. Call Close to stop it. A nil logger
// discards diagnostics; a nil runner disables kernel command execution
// (cmd nodes then sit [ready] for pull mode, like everything else).
func NewScheduler(store Store, clock ports.Clock, wake WakeFunc, run CmdRunner, logger *slog.Logger) *Scheduler {
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(io.Discard, nil))
	}
	s := &Scheduler{
		store:  store,
		clock:  clock,
		wake:   wake,
		run:    run,
		log:    logger,
		ticker: time.NewTicker(scanInterval),
		kick:   make(chan struct{}, 1),
		stop:   make(chan struct{}),
		done:   make(chan struct{}),
	}
	go s.loop()
	return s
}

// Kick asks the drain loop to scan now instead of at the next tick.
// Non-blocking and coalescing.
func (s *Scheduler) Kick() {
	select {
	case s.kick <- struct{}{}:
	default:
	}
}

// Close stops the scan loop and waits for it to exit. In-flight wakes are not
// interrupted — they are sessions, and sessions outlive their summoner.
func (s *Scheduler) Close() {
	close(s.stop)
	<-s.done
}

func (s *Scheduler) loop() {
	defer close(s.done)
	defer s.ticker.Stop()
	for {
		select {
		case <-s.stop:
			return
		case <-s.ticker.C:
			s.drain(context.Background())
		case <-s.kick:
			s.drain(context.Background())
		}
	}
}

// drain advances the forest until nothing more can fire. Commands launch
// concurrently (equal-Seq groups are the fan-out shape; their completions
// Kick the loop, which is how a chain flows at execution speed). Wakes run
// serially in due order on this goroutine, so firings reach the user in the
// order their intent sequenced them, and no two scans can ever race the same
// node — every launch passed a compare-and-claim or compare-and-disarm in
// collect. The cost of serial wakes is that one slow turn delays later
// firings; for an agent's obligations, ordered-and-late beats
// concurrent-and-shuffled.
func (s *Scheduler) drain(ctx context.Context) {
	for {
		cmds, wakes := s.collect(ctx)
		if len(cmds) == 0 && len(wakes) == 0 {
			return
		}
		for _, n := range cmds {
			go s.runCmdNode(ctx, n)
		}
		for _, n := range wakes {
			select {
			case <-s.stop:
				return
			default:
			}
			s.log.Info("plan scheduler: firing", "node", n.ID, "goal", n.Goal)
			if err := s.wake(ctx, WakeEvent{Node: n, Reason: WakeDue}); err != nil {
				s.log.Warn("plan scheduler: wake", "node", n.ID, "err", err)
			}
		}
	}
}

// collect claims everything fireable right now and returns it. All state
// transitions persist BEFORE any execution — compare-and-disarm for time
// arming, compare-and-claim for command nodes — so at most one process fires
// a node per arming no matter how many schedulers share the store, and a
// failed launch degrades to pull mode rather than re-firing every tick.
func (s *Scheduler) collect(ctx context.Context) (cmds, wakes []Node) {
	now := s.clock.Now()
	nodes, err := s.store.OpenPlanNodes(ctx)
	if err != nil {
		s.log.Warn("plan scheduler: scan", "err", err)
		return nil, nil
	}
	cmdsAll, wakesAll := Fireable(nodes, now)
	if len(cmdsAll) > 0 && s.run == nil {
		cmdsAll = nil // no runner wired: cmd nodes stay [ready] for pull mode
	}
	for _, n := range cmdsAll {
		if !s.disarm(ctx, &n, now) {
			continue
		}
		won, err := s.store.ClaimPlanNode(ctx, n.ID, "kernel")
		if err != nil || !won {
			continue
		}
		n.Status = StatusActive
		n.Owner = "kernel"
		cmds = append(cmds, n)
	}
	for _, n := range wakesAll {
		if s.disarm(ctx, &n, now) {
			wakes = append(wakes, n)
		}
	}
	return cmds, wakes
}

// disarm clears or advances a node's time arming via compare-and-disarm,
// reporting whether this scheduler won the firing. Un-armed nodes (a plain
// ready cmd in a pipeline) trivially win. n's arming fields are updated in
// place on success so later writes do not resurrect the old arming.
func (s *Scheduler) disarm(ctx context.Context, n *Node, now time.Time) bool {
	if n.Kind != KindMaintain && n.After.IsZero() {
		return true
	}
	after, nextDue := time.Time{}, n.NextDue
	if n.Kind == KindMaintain {
		nextDue = now.Add(n.Every)
	}
	won, err := s.store.DisarmPlanNode(ctx, *n, after, nextDue)
	if err != nil {
		s.log.Warn("plan scheduler: disarm", "node", n.ID, "err", err)
		return false
	}
	if won {
		n.After, n.NextDue = after, nextDue
	}
	return won
}

// cmdTailLimit bounds how much command output rides into outcomes and wake
// prompts.
const cmdTailLimit = 1200

// runCmdNode executes one claimed command node to its verdict: run as a job,
// map the exit code, record the attempt (VerifiedBy "exit" — mechanical
// ground truth, the top of the trust hierarchy), and Kick the loop so a
// successor fires immediately. A failure summons the model with the log tail
// — the exception handler, the only model turn a healthy pipeline never
// spends.
func (s *Scheduler) runCmdNode(ctx context.Context, n Node) {
	s.log.Info("plan scheduler: running", "node", n.ID, "cmd", n.Cmd)
	jobID, code, tail, err := s.run(ctx, n.Cmd)
	if len(tail) > cmdTailLimit {
		tail = "…" + tail[len(tail)-cmdTailLimit:]
	}
	if jobID != "" {
		n.Owner = "job:" + jobID
	}

	success := err == nil && code == 0
	outcome := fmt.Sprintf("exit %d", code)
	if err != nil {
		outcome = "run failed: " + err.Error()
	}
	verdict := VerdictSuccess
	if !success {
		verdict = VerdictFail
		if tail != "" {
			outcome += " — " + tail
		}
	}

	switch {
	case n.Kind == KindMaintain:
		n.Status = StatusPending // standing obligations have no terminal
	case success:
		n.Status, n.Outcome = StatusDone, outcome
	default:
		n.Status, n.Outcome = StatusFailed, outcome
	}
	if err := s.store.UpdatePlanNode(ctx, n); err != nil {
		s.log.Warn("plan scheduler: record cmd", "node", n.ID, "err", err)
		return
	}
	if err := s.store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: n.Owner, Verdict: verdict, Outcome: outcome, VerifiedBy: "exit"}); err != nil {
		s.log.Warn("plan scheduler: attempt", "node", n.ID, "err", err)
	}
	if !success {
		if err := s.wake(ctx, WakeEvent{Node: n, Reason: WakeCmdFailed, Detail: tail}); err != nil {
			s.log.Warn("plan scheduler: failure wake", "node", n.ID, "err", err)
		}
	}
	s.Kick()
}

// Fireable partitions the work the kernel can advance right now. cmds are
// ready nodes with a mechanical payload — time-armed or not, they run as jobs
// with no model turn, in input order (sibling order), and equal-Seq groups
// launch together. wakes are time-armed judgment nodes whose moment arrived —
// they need a model session, in due order (then ID) so firings reach the user
// in the order their intent sequenced them. Pure, so firing policy is
// testable with a fake clock.
func Fireable(nodes []Node, now time.Time) (cmds, wakes []Node) {
	for _, n := range nodes {
		if !Ready(n, GatedBySibling(nodes, n), now) {
			continue
		}
		switch {
		case n.Cmd != "":
			cmds = append(cmds, n)
		case n.Kind == KindMaintain || !n.After.IsZero():
			wakes = append(wakes, n)
		}
	}
	sort.Slice(wakes, func(a, b int) bool {
		ta, tb := dueAt(wakes[a]), dueAt(wakes[b])
		if !ta.Equal(tb) {
			return ta.Before(tb)
		}
		return wakes[a].ID < wakes[b].ID
	})
	return cmds, wakes
}

// dueAt is the instant a time-armed node became (or becomes) due.
func dueAt(n Node) time.Time {
	if n.Kind == KindMaintain {
		return n.NextDue
	}
	return n.After
}
