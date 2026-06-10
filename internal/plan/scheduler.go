package plan

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"sort"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/ports"
)

// The scheduler is the host's clock: the one place where time initiates work
// instead of responding to it. Each scan advances the forest's fireable
// nodes — cmd nodes it runs as jobs, time-armed and callback judgment nodes
// it wakes an executor for — disarming each before acting so a firing happens
// once. Everything mechanical lives here, deterministically (the kernel side
// of the kernel/judgment split); what to do upon waking is the model's, in
// the session the wake spawns.
//
// It fires only triggered nodes (Node.Armed) and ready cmd nodes. A plain
// ready judgment node with no trigger is pull mode's job — the projection
// marks it and a running turn picks it up; driving a whole plan unprompted is
// the future driver's job. The scheduler is just the part of the agent that
// owns a watch.
//
// Durability comes from the store, not the process: triggers are rows, so a
// restarted host resumes where the dead one stood, and a firing missed while
// down coalesces into one wake on the next scan (the re-arm is measured from
// now, never replayed per missed period). Concurrency across processes is
// safe by construction: every firing passes a compare-and-claim (cmd) or
// compare-and-disarm (wake) UPDATE, so when an interactive session and the
// serve host both tick the same store, exactly one wins each node.

// WakeReason says why the model is being summoned.
type WakeReason string

const (
	// WakeDue: a time-armed judgment node's moment arrived.
	WakeDue WakeReason = "due"
	// WakeCmdFailed: a kernel-run command exited non-zero — the exception
	// handler case.
	WakeCmdFailed WakeReason = "cmd_failed"
	// WakeReady: a callback node's prerequisites finished (WakeOnReady) — the
	// completion-report case.
	WakeReady WakeReason = "ready"
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
// any other job); the scheduler stays free of process machinery. Nil disables
// the shell executor (e.g. approval mode), leaving shell nodes [ready] for a
// gated turn.
type CmdRunner func(ctx context.Context, command string) (jobID string, exitCode int, tail string, err error)

// NotifyFunc delivers a node's message to the user (the outbox). The host
// supplies it; the scheduler stays free of the outbox. It is the second
// mechanical executor — "remind me", "tell me when X" — discharged with no
// model turn.
type NotifyFunc func(ctx context.Context, msg, session string) error

// scanInterval is how often the scheduler checks for due nodes. Recurrence
// periods are minutes-to-hours; a coarse scan keeps the idle host at one cheap
// query per tick instead of a precise per-node timer heap it doesn't need yet.
const scanInterval = 30 * time.Second

// maxConcurrentCmds caps how many kernel cmd jobs run at once, so a large
// model-authored parallel group cannot spawn unbounded concurrent processes.
// Excess ready cmds wait for a free slot and are claimed on a later scan (a
// finishing job Kicks the loop), preserving order within the cap.
const maxConcurrentCmds = 8

// staleClaimAge is how long a kernel-owned active node may sit before the
// scheduler reclaims it as an orphan. It must exceed the longest a live cmd
// job can hold a node active — the cmd job timeout (piwire.cmdJobTimeout,
// 30m) plus margin — so a still-running job is never mistaken for a dead one.
const staleClaimAge = 45 * time.Minute

// closeGrace bounds how long Close waits for in-flight cmd goroutines to
// record their results after cancellation, so shutdown is prompt even if a
// final store write is slow.
const closeGrace = 5 * time.Second

// Scheduler owns the scan loop. One goroutine per process owns firing, so
// disarm-then-act never races itself within a process; across processes the
// store's compare-and-claim/disarm UPDATEs settle who fires each node.
type Scheduler struct {
	store  Store
	clock  ports.Clock
	wake   WakeFunc
	run    CmdRunner
	notify NotifyFunc
	log    *slog.Logger
	ticker *time.Ticker
	// baseCtx is the parent of every wake turn and cmd job; cancelling it (via
	// Close) interrupts in-flight model turns and kills running cmd jobs so
	// shutdown is prompt instead of blocking on a 15-minute wake. cmd nodes a
	// shutdown kills are reclaimed back to pending on the next start.
	baseCtx context.Context
	cancel  context.CancelFunc
	// sem caps concurrent kernel cmd jobs (a slot is held from claim through
	// the job's completion). wg tracks the cmd goroutines so Close can wait
	// for their final store writes within closeGrace.
	sem chan struct{}
	wg  sync.WaitGroup
	// kick wakes the drain loop ahead of the next tick — a finished command
	// may have just ungated its successor, and a four-step pipeline should
	// flow at execution speed, not at 30-second tick quanta.
	kick chan struct{}
	stop chan struct{}
	done chan struct{}
}

// NewScheduler starts the scan loop. Call Close to stop it. A nil logger
// discards diagnostics; a nil runner disables the shell executor (shell nodes
// sit [ready] for a gated turn); a nil notify disables the notify executor.
func NewScheduler(store Store, clock ports.Clock, wake WakeFunc, run CmdRunner, notify NotifyFunc, logger *slog.Logger) *Scheduler {
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(io.Discard, nil))
	}
	ctx, cancel := context.WithCancel(context.Background())
	s := &Scheduler{
		store:   store,
		clock:   clock,
		wake:    wake,
		run:     run,
		notify:  notify,
		log:     logger,
		ticker:  time.NewTicker(scanInterval),
		baseCtx: ctx,
		cancel:  cancel,
		sem:     make(chan struct{}, maxConcurrentCmds),
		kick:    make(chan struct{}, 1),
		stop:    make(chan struct{}),
		done:    make(chan struct{}),
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

// Close stops the scan loop and cancels in-flight work, then waits — bounded
// by closeGrace — for cmd goroutines to record their results before the store
// is closed under them. A cancelled wake turn ends promptly (its context is a
// child of baseCtx), so Close no longer blocks on a long-running model turn.
func (s *Scheduler) Close() {
	s.cancel()
	close(s.stop)
	<-s.done
	waited := make(chan struct{})
	go func() { s.wg.Wait(); close(waited) }()
	select {
	case <-waited:
	case <-time.After(closeGrace):
	}
}

func (s *Scheduler) loop() {
	defer close(s.done)
	defer s.ticker.Stop()
	for {
		select {
		case <-s.stop:
			return
		case <-s.ticker.C:
			s.drain(s.baseCtx)
		case <-s.kick:
			s.drain(s.baseCtx)
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
		mech, wakes := s.collect(ctx)
		if len(mech) == 0 && len(wakes) == 0 {
			return
		}
		for _, n := range mech {
			s.wg.Add(1)
			go s.runMechanical(n)
		}
		for _, w := range wakes {
			select {
			case <-s.stop:
				return
			default:
			}
			s.log.Info("plan scheduler: firing", "node", w.Node.ID, "goal", w.Node.Goal, "reason", string(w.Reason))
			if err := s.wake(ctx, w); err != nil {
				s.log.Warn("plan scheduler: wake", "node", w.Node.ID, "err", err)
			}
		}
	}
}

// collect claims everything fireable right now and returns it. All state
// transitions persist BEFORE any execution — compare-and-disarm for time
// arming, compare-and-claim for command nodes — so at most one process fires
// a node per arming no matter how many schedulers share the store, and a
// failed launch degrades to pull mode rather than re-firing every tick.
func (s *Scheduler) collect(ctx context.Context) (mech []Node, wakes []WakeEvent) {
	now := s.clock.Now()
	// Recover orphans first: a kernel-owned node still active past staleClaimAge
	// is from a host that died mid-run; reset it to pending so it (and its
	// gated successors) can fire again instead of wedging forever.
	if n, err := s.store.ReclaimStaleKernelNodes(ctx, now.Add(-staleClaimAge)); err != nil {
		s.log.Warn("plan scheduler: reclaim", "err", err)
	} else if n > 0 {
		s.log.Info("plan scheduler: reclaimed orphaned cmd nodes", "count", n)
	}
	nodes, err := s.store.OpenPlanNodes(ctx)
	if err != nil {
		s.log.Warn("plan scheduler: scan", "err", err)
		return nil, nil
	}
	mechAll, wakesAll := Fireable(nodes, now)
	for _, n := range mechAll {
		// Skip executors with no runner wired: shell under approval (gated
		// turn runs it instead), notify with no outbox. The node stays [ready].
		if !s.runnable(n) {
			continue
		}
		// Hold a concurrency slot before claiming, so a claimed node always
		// runs (and releases the slot). When the cap is full, stop claiming —
		// the rest fire on a later scan, which a finishing job Kicks.
		if !s.acquire() {
			break
		}
		if !s.disarm(ctx, &n, now) {
			s.release()
			continue
		}
		won, err := s.store.ClaimPlanNode(ctx, n.ID, "kernel")
		if err != nil || !won {
			s.release()
			continue
		}
		n.Status = StatusActive
		n.Owner = "kernel"
		mech = append(mech, n)
	}
	// The reason was stamped by Fireable from the un-mutated fields; disarm
	// then zeroes them, so it must never be re-derived here (the bug that made
	// every deferred task fire with the callback prompt).
	for _, w := range wakesAll {
		n := w.Node
		if s.disarm(ctx, &n, now) {
			wakes = append(wakes, w)
		}
	}
	return mech, wakes
}

// runnable reports whether the executor for a mechanical node has its runner
// wired. Shell needs a CmdRunner (nil under approval); notify needs a
// NotifyFunc. An unrunnable node is left [ready] rather than claimed.
func (s *Scheduler) runnable(n Node) bool {
	switch n.Executor() {
	case ExecShell:
		return s.run != nil
	case ExecNotify:
		return s.notify != nil
	case ExecAgent, ExecAsk:
		return false // not mechanical: agent wakes a session, ask waits on the user
	}
	return false
}

// disarm clears or advances a node's triggers via compare-and-disarm,
// reporting whether this scheduler won the firing. Time arming clears or
// re-arms (maintain); the WakeOnReady callback flag is one-shot and always
// clears. Un-triggered nodes (a plain ready cmd in a pipeline) trivially win.
// n's trigger fields are updated in place on success so later writes do not
// resurrect the old arming.
func (s *Scheduler) disarm(ctx context.Context, n *Node, now time.Time) bool {
	if !n.Armed() {
		return true
	}
	after, nextDue := time.Time{}, n.NextDue
	if n.Recurring() {
		nextDue = now.Add(n.Every)
	}
	won, err := s.store.DisarmPlanNode(ctx, *n, after, nextDue, false)
	if err != nil {
		s.log.Warn("plan scheduler: disarm", "node", n.ID, "err", err)
		return false
	}
	if won {
		n.After, n.NextDue, n.WakeOnReady = after, nextDue, false
	}
	return won
}

// cmdTailLimit bounds how much command output rides into outcomes and wake
// prompts.
const cmdTailLimit = 1200

// acquire takes a cmd concurrency slot without blocking, reporting success.
func (s *Scheduler) acquire() bool {
	select {
	case s.sem <- struct{}{}:
		return true
	default:
		return false
	}
}

func (s *Scheduler) release() { <-s.sem }

// runMechanical discharges one claimed mechanical node — the kernel doing
// work with no model turn — and Kicks the loop so a successor fires
// immediately. It dispatches on the node's executor: shell runs a job, notify
// emits to the outbox. Both release the concurrency slot and the wait-group
// (Close waits within closeGrace).
func (s *Scheduler) runMechanical(n Node) {
	defer s.wg.Done()
	defer s.release()
	defer s.Kick()
	switch n.Executor() {
	case ExecShell:
		s.runShell(n)
	case ExecNotify:
		s.runNotify(n)
	case ExecAgent, ExecAsk:
		// Not mechanical — never collected into mech; present for exhaustiveness.
	}
}

// runNotify delivers a notify node's message and records it done — the
// mechanical answer to "remind me" / "tell me when X", with no session spawned.
func (s *Scheduler) runNotify(n Node) {
	if s.baseCtx.Err() != nil {
		return
	}
	ctx := context.Background()
	outcome := "notified"
	verdict := VerdictSuccess
	if err := s.notify(ctx, n.Notify, n.Owner); err != nil {
		outcome, verdict = "notify failed: "+err.Error(), VerdictFail
		s.log.Warn("plan scheduler: notify", "node", n.ID, "err", err)
	}
	// A recurring notify (a standing ping) returns to pending; a one-shot ends.
	status := StatusDone
	if n.Recurring() {
		status = StatusPending
	}
	if err := s.store.SetPlanNodeStatus(ctx, n.ID, status, outcome, n.Owner); err != nil {
		s.log.Warn("plan scheduler: record notify", "node", n.ID, "err", err)
		return
	}
	if err := s.store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: n.Owner, Verdict: verdict, Outcome: outcome, VerifiedBy: "kernel"}); err != nil {
		s.log.Warn("plan scheduler: attempt", "node", n.ID, "err", err)
	}
}

// runShell executes one claimed command node to its verdict: run as a job,
// map the exit code, record the attempt (VerifiedBy "exit" — mechanical
// ground truth, the top of the trust hierarchy). A failure summons the model
// with the log tail — the exception handler, the only model turn a healthy
// pipeline never spends. The command runs under baseCtx so Close kills it; the
// result is written with a detached context so a normally-finishing job still
// records even as shutdown begins (Close waits for these within closeGrace). A
// job Close kills leaves the node active, reclaimed to pending on next start.
func (s *Scheduler) runShell(n Node) {
	s.log.Info("plan scheduler: running", "node", n.ID, "cmd", n.Cmd)
	jobID, code, tail, err := s.run(s.baseCtx, n.Cmd)
	if s.baseCtx.Err() != nil {
		return // shut down mid-run: leave active for reclaim, don't write to a closing store
	}
	ctx := context.Background()
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

	// A recurring cmd node has no terminal — it returns to pending for its next
	// firing; a one-shot cmd node lands on its verdict. The kernel does not
	// consult CanTransition: a process's exit code is ground truth, not a
	// state the model may veto.
	status := StatusFailed
	switch {
	case n.Recurring():
		status = StatusPending
	case success:
		status = StatusDone
	}
	if err := s.store.SetPlanNodeStatus(ctx, n.ID, status, outcome, n.Owner); err != nil {
		s.log.Warn("plan scheduler: record cmd", "node", n.ID, "err", err)
		return
	}
	if err := s.store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: n.Owner, Verdict: verdict, Outcome: outcome, VerifiedBy: "exit"}); err != nil {
		s.log.Warn("plan scheduler: attempt", "node", n.ID, "err", err)
	}
	if !success {
		n.Status, n.Outcome = status, outcome
		if err := s.wake(ctx, WakeEvent{Node: n, Reason: WakeCmdFailed, Detail: tail}); err != nil {
			s.log.Warn("plan scheduler: failure wake", "node", n.ID, "err", err)
		}
	}
}

// Fireable partitions the work the kernel can advance right now along the
// executor axis: mech are mechanical nodes (shell, notify) the kernel runs
// itself with no model turn (time-armed or not; equal-Seq groups launch
// together); wakes are triggered judgment nodes the kernel must wake a model
// for. Each wake's reason is classified here from the un-mutated fields — the
// one place it is decided, so a later disarm cannot invalidate it (a deferred
// timer is WakeDue; a callback whose gates cleared is WakeReady). wakes are
// ordered by due time then ID so firings reach the user in the order their
// intent sequenced them. Pure, so firing and classification are testable with
// a fake clock.
func Fireable(nodes []Node, now time.Time) (mech []Node, wakes []WakeEvent) {
	for _, n := range nodes {
		if !Ready(n, GatedBySibling(nodes, n), now) {
			continue
		}
		if n.Mechanical() {
			mech = append(mech, n)
			continue
		}
		// Judgment node: only the scheduler-triggered ones fire here; a plain
		// ready agent node is pull mode's (a running turn picks it up).
		switch {
		case n.Recurring() || !n.After.IsZero():
			wakes = append(wakes, WakeEvent{Node: n, Reason: WakeDue})
		case n.WakeOnReady:
			wakes = append(wakes, WakeEvent{Node: n, Reason: WakeReady})
		}
	}
	sort.Slice(wakes, func(a, b int) bool {
		ta, tb := dueAt(wakes[a].Node), dueAt(wakes[b].Node)
		if !ta.Equal(tb) {
			return ta.Before(tb)
		}
		return wakes[a].Node.ID < wakes[b].Node.ID
	})
	return mech, wakes
}

// dueAt is the instant a time-armed node became (or becomes) due.
func dueAt(n Node) time.Time {
	if n.Recurring() {
		return n.NextDue
	}
	return n.After
}
