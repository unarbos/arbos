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
	// WakeCondition: a gated node's shell predicate held — the world reached
	// the state the node was watching for, so act on the goal.
	WakeCondition WakeReason = "condition"
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
// node names the firing plan node, so the host can stamp the message's
// coalescing source: a recurring node's new firing supersedes its own
// undelivered predecessor instead of piling up while no door is open.
type NotifyFunc func(ctx context.Context, msg, session string, node NodeID) error

// scanInterval is how often the scheduler checks for due nodes. Recurrence
// periods are minutes-to-hours; a coarse scan keeps the idle host at one cheap
// query per tick instead of a precise per-node timer heap it doesn't need yet.
const scanInterval = 30 * time.Second

// maxConcurrentCmds caps how many kernel cmd jobs run at once, so a large
// model-authored parallel group cannot spawn unbounded concurrent processes.
// Excess ready cmds wait for a free slot and are claimed on a later scan (a
// finishing job Kicks the loop), preserving order within the cap.
const maxConcurrentCmds = 8

// maxConcurrentWakes caps concurrent agent wakes (model turns). It is separate
// from maxConcurrentCmds on purpose: a wake is a provider round-trip with a
// rate-limit profile nothing like a local process, so the two resources are
// bounded independently (the concurrency corollary — isolate state with a
// different contention shape). A parallel group of agent nodes fans out up to
// this many at once; the rest fire on later scans as slots free (a finishing
// wake Kicks the loop), the same back-pressure cmds already get.
const maxConcurrentWakes = 4

// staleClaimAge is how long a kernel-owned active node may sit before the
// scheduler reclaims it as an orphan. It must exceed the longest a live cmd
// job can hold a node active — the cmd job timeout (piwire.cmdJobTimeout,
// 30m) plus margin — so a still-running job is never mistaken for a dead one.
const staleClaimAge = 45 * time.Minute

// staleWakeAge is the DEFAULT for how long an agent node claimed by a
// scheduler wake may sit active before it is reclaimed as an orphan from a host
// that died mid-wake. A wake turn is bounded (piwire.wakeTurnTimeout, 15m), so
// anything still active well past that is dead; the wide margin keeps a
// slow-but-live wake safe. The host raises it via WithReclaimAfter when it
// raises the wake timeout (ARBOS_WAKE_TIMEOUT), so the reclaim margin always
// exceeds the longest a live wake can run — otherwise the reclaim would reset a
// node out from under a running agent.
const staleWakeAge = 60 * time.Minute

// failNoteInterval coalesces the "a scheduled run is failing" notice per node:
// a sustained provider outage that fails every firing pings the user once per
// interval, not on every scan, so an overnight failure surfaces without spam.
const failNoteInterval = 30 * time.Minute

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
	// sem caps concurrent kernel cmd jobs; wakeSem caps concurrent agent
	// wakes (model turns). A slot is held from claim/launch through the
	// work's completion. wg tracks BOTH the cmd and wake goroutines so Close
	// can wait for their final store writes within closeGrace.
	sem     chan struct{}
	wakeSem chan struct{}
	wg      sync.WaitGroup
	// inflight is the set of nodes whose agent wake is still running, so a
	// recurring node cannot re-fire and overlap itself mid-wake (its trigger
	// re-arms at fire time, which would otherwise become due again before a
	// slow turn finishes). Process-local: across processes, compare-and-disarm
	// still gives per-arming safety; this guards the re-arm within one host.
	inflightMu sync.Mutex
	inflight   map[NodeID]struct{}
	// failNote records when each node last emitted a wake-failure notice, so a
	// sustained outage coalesces into one ping per failNoteInterval rather than
	// one per firing. Process-local: a best-effort heads-up, not durable state.
	// Pruned when a node's wake succeeds, so it tracks only currently-failing
	// nodes rather than growing for the host's lifetime.
	failMu   sync.Mutex
	failNote map[NodeID]time.Time
	// reclaimAfter is how long an agent node may sit active before it is
	// reclaimed as a wake orphan; the host couples it to the wake timeout.
	reclaimAfter time.Duration
	// kick wakes the drain loop ahead of the next tick — a finished command
	// may have just ungated its successor, and a four-step pipeline should
	// flow at execution speed, not at 30-second tick quanta.
	kick chan struct{}
	stop chan struct{}
	done chan struct{}
}

// SchedulerOption tunes an optional scheduler knob without widening
// NewScheduler's signature.
type SchedulerOption func(*Scheduler)

// WithReclaimAfter overrides how long an agent-claimed node may sit active
// before it is reclaimed as a wake orphan. The host sets it from the wake turn
// timeout (ARBOS_WAKE_TIMEOUT) plus a margin, so the reclaim window always
// exceeds the longest a live wake can run — a non-positive value keeps the
// default (staleWakeAge).
func WithReclaimAfter(d time.Duration) SchedulerOption {
	return func(s *Scheduler) {
		if d > 0 {
			s.reclaimAfter = d
		}
	}
}

// NewScheduler starts the scan loop. Call Close to stop it. A nil logger
// discards diagnostics; a nil runner disables the shell executor (shell nodes
// sit [ready] for a gated turn); a nil notify disables the notify executor.
func NewScheduler(store Store, clock ports.Clock, wake WakeFunc, run CmdRunner, notify NotifyFunc, logger *slog.Logger, opts ...SchedulerOption) *Scheduler {
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(io.Discard, nil))
	}
	ctx, cancel := context.WithCancel(context.Background())
	s := &Scheduler{
		store:        store,
		clock:        clock,
		wake:         wake,
		run:          run,
		notify:       notify,
		log:          logger,
		ticker:       time.NewTicker(scanInterval),
		baseCtx:      ctx,
		cancel:       cancel,
		sem:          make(chan struct{}, maxConcurrentCmds),
		wakeSem:      make(chan struct{}, maxConcurrentWakes),
		inflight:     make(map[NodeID]struct{}),
		failNote:     make(map[NodeID]time.Time),
		reclaimAfter: staleWakeAge,
		kick:         make(chan struct{}, 1),
		stop:         make(chan struct{}),
		done:         make(chan struct{}),
	}
	for _, opt := range opts {
		opt(s)
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

// drain advances the forest until nothing more can fire. Both axes launch
// concurrently under their own caps: commands (equal-Seq groups are the
// fan-out shape) and agent wakes (a parallel group of agent nodes fans out up
// to maxConcurrentWakes at once). collect holds a slot for everything it
// returns and disarms/claims it, so a launched node never re-collects and the
// outer loop cannot spin — a full cap simply returns nothing until a finishing
// job or wake Kicks the loop. Wakes are claimed in due order, so the user's
// intent sequences which fire first; completions interleave, which is what
// fan-out wants.
func (s *Scheduler) drain(ctx context.Context) {
	for {
		select {
		case <-s.stop:
			return
		default:
		}
		mech, conds, wakes := s.collect(ctx)
		if len(mech) == 0 && len(conds) == 0 && len(wakes) == 0 {
			return
		}
		for _, n := range mech {
			s.wg.Add(1)
			go s.runMechanical(n)
		}
		for _, n := range conds {
			s.wg.Add(1)
			go s.runCondition(n)
		}
		for _, w := range wakes {
			s.wg.Add(1)
			go s.runWake(ctx, w)
		}
	}
}

// runCondition evaluates a gated node's shell predicate and, only if it holds
// (exit 0), discharges the node's Do — a notify emitted inline or an agent
// wake. A non-zero exit re-arms quietly: no attempt recorded, no model turn,
// so a watch that polls all day costs only its cheap predicate until the world
// reaches the state it is waiting for. Recurring gated nodes return to pending
// to keep watching (the agent the wake spawns can cancel the node if a single
// trip was the point). The predicate holds a cmd slot for its run; a
// triggered agent wake runs inline under that slot, so condition-driven wakes
// are bounded by the cmd cap rather than the wake cap — acceptable since the
// poll, not the rare trip, is the steady-state cost.
func (s *Scheduler) runCondition(n Node) {
	defer s.recoverFiring("condition", n.ID)
	defer s.wg.Done()
	defer s.release()
	defer s.Kick()
	s.log.Info("plan scheduler: evaluating", "node", n.ID, "cond", n.Cond)
	_, code, tail, err := s.run(s.baseCtx, n.Cond)
	if s.baseCtx.Err() != nil {
		return // shutting down: leave active for reclaim
	}
	ctx := context.Background()
	held := err == nil && code == 0

	// Recurring gated nodes keep watching; a one-shot ends once it has fired.
	status := StatusPending
	if !n.Recurring() && held {
		status = StatusDone
	}
	if e := s.store.SetPlanNodeStatus(ctx, n.ID, status, "", n.Owner); e != nil {
		s.log.Warn("plan scheduler: record condition", "node", n.ID, "err", e)
		return
	}
	if !held {
		return // predicate false this period — no fire, no attempt
	}
	if len(tail) > cmdTailLimit {
		tail = "…" + tail[len(tail)-cmdTailLimit:]
	}
	outcome := "condition held"
	if tail != "" {
		outcome += ": " + tail
	}
	if e := s.store.AddPlanAttempt(ctx, Attempt{Node: n.ID, Session: n.Owner, Verdict: VerdictSuccess, Outcome: outcome, VerifiedBy: "exit"}); e != nil {
		s.log.Warn("plan scheduler: attempt", "node", n.ID, "err", e)
	}
	// Discharge the Do. A notify node fires its message; otherwise the gate
	// summons the model with the goal (and the predicate's output as detail).
	if n.Executor() == ExecNotify {
		if s.notify == nil {
			return
		}
		target := n.Origin
		if target == "" {
			target = n.Owner
		}
		if e := s.notify(ctx, n.Notify, target, n.ID); e != nil {
			s.log.Warn("plan scheduler: condition notify", "node", n.ID, "err", e)
		}
		return
	}
	n.Status = StatusPending
	s.log.Info("plan scheduler: firing", "node", n.ID, "goal", n.Goal, "reason", string(WakeCondition))
	if e := s.wake(ctx, WakeEvent{Node: n, Reason: WakeCondition, Detail: tail}); e != nil {
		s.log.Warn("plan scheduler: condition wake", "node", n.ID, "err", e)
	}
}

// recoverFiring stops a panic in one fired node from killing the scan loop:
// the firing goroutines run model turns and shell predicates, and a panic in
// either must degrade that one firing, not decapitate the host's clock. It is
// the scheduler-side analog of the engine's per-turn recover.
func (s *Scheduler) recoverFiring(what string, node NodeID) {
	if r := recover(); r != nil {
		s.log.Error("plan scheduler: panic in firing", "what", what, "node", node, "panic", r)
	}
}

// runWake discharges one claimed agent wake in its own goroutine — a model
// turn the scheduler summoned. It releases the wake slot and clears the
// node's in-flight mark on completion, then Kicks so a freed slot picks up
// the next due wake immediately rather than at the next tick.
func (s *Scheduler) runWake(ctx context.Context, w WakeEvent) {
	defer s.recoverFiring("wake", w.Node.ID)
	defer s.wg.Done()
	defer s.releaseWake()
	defer s.clearInflight(w.Node.ID)
	defer s.Kick()
	s.log.Info("plan scheduler: firing", "node", w.Node.ID, "goal", w.Node.Goal, "reason", string(w.Reason))
	if err := s.wake(ctx, w); err != nil {
		s.log.Warn("plan scheduler: wake", "node", w.Node.ID, "err", err)
		// The wake's turn exhausted its retries and fallback models (engine
		// resilience) or hit its deadline. A recurring node re-fires next
		// period and a deferred one is already reclaimable, so the work is not
		// lost — but a sustained outage would otherwise fail silently all
		// night. Surface a coalesced heads-up so the user learns the agent is
		// stuck, without spamming on every firing.
		s.noteWakeFailure(w.Node, err)
		return
	}
	// A clean firing clears any prior failure note for this node, so the map
	// tracks only nodes that are currently failing and a recovered-then-failing
	// node pings again immediately rather than waiting out the interval.
	s.clearFailNote(w.Node.ID)
}

// noteWakeFailure emits at most one wake-failure notice per node per
// failNoteInterval, routed to the chat that owns the node (its Origin). It is
// best-effort: no notify wired (no outbox), a shutting-down host, or a missing
// target all silently skip — the failure is already logged.
func (s *Scheduler) noteWakeFailure(n Node, cause error) {
	if s.notify == nil || s.baseCtx.Err() != nil {
		return
	}
	now := s.clock.Now()
	s.failMu.Lock()
	last, seen := s.failNote[n.ID]
	if seen && now.Sub(last) < failNoteInterval {
		s.failMu.Unlock()
		return
	}
	s.failNote[n.ID] = now
	s.failMu.Unlock()

	target := n.Origin
	if target == "" {
		target = n.Owner
	}
	if target == "" {
		return
	}
	msg := fmt.Sprintf("Heads up: a scheduled task keeps failing — node #%d (%s): %v. I'll keep retrying on its schedule.", n.ID, n.Goal, cause)
	if e := s.notify(context.Background(), msg, target, n.ID); e != nil {
		s.log.Warn("plan scheduler: wake-failure notify", "node", n.ID, "err", e)
	}
}

// clearFailNote drops a node's coalescing record after a clean firing, so the
// failNote map holds only currently-failing nodes instead of growing for the
// host's lifetime.
func (s *Scheduler) clearFailNote(id NodeID) {
	s.failMu.Lock()
	delete(s.failNote, id)
	s.failMu.Unlock()
}

// collect claims everything fireable right now and returns it. All state
// transitions persist BEFORE any execution — compare-and-disarm for time
// arming, compare-and-claim for command nodes — so at most one process fires
// a node per arming no matter how many schedulers share the store, and a
// failed launch degrades to pull mode rather than re-firing every tick.
func (s *Scheduler) collect(ctx context.Context) (mech, conds []Node, wakes []WakeEvent) {
	now := s.clock.Now()
	// Recover orphans first: a kernel-owned node still active past staleClaimAge
	// is from a host that died mid-run; reset it to pending so it (and its
	// gated successors) can fire again instead of wedging forever.
	if n, err := s.store.ReclaimStaleKernelNodes(ctx, now.Add(-staleClaimAge)); err != nil {
		s.log.Warn("plan scheduler: reclaim", "err", err)
	} else if n > 0 {
		s.log.Info("plan scheduler: reclaimed orphaned cmd nodes", "count", n)
	}
	// Recover agent wakes orphaned by a host that died mid-turn too: a
	// scheduler-spawned session that never recorded its node's outcome leaves
	// it active forever, wedging the node (and its gated successors). Scoped to
	// scheduler-origin sessions and a margin past the wake timeout, so a live
	// interactive session's active node is never disturbed.
	if n, err := s.store.ReclaimStaleAgentNodes(ctx, now.Add(-s.reclaimAfter)); err != nil {
		s.log.Warn("plan scheduler: reclaim wakes", "err", err)
	} else if n > 0 {
		s.log.Info("plan scheduler: reclaimed orphaned wake nodes", "count", n)
	}
	nodes, err := s.store.OpenPlanNodes(ctx)
	if err != nil {
		s.log.Warn("plan scheduler: scan", "err", err)
		return nil, nil, nil
	}
	mechAll, condsAll, wakesAll := Fireable(nodes, now)
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
	// Gated nodes are evaluated like cmd nodes (the predicate is a shell run),
	// so they take a cmd slot and a kernel claim — held through the predicate
	// and, if it holds, the Do it discharges. Skipped when no CmdRunner is
	// wired (approval mode); the node stays [ready] to evaluate later.
	for _, n := range condsAll {
		if s.run == nil {
			continue
		}
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
		conds = append(conds, n)
	}
	// The reason was stamped by Fireable from the un-mutated fields; disarm
	// then zeroes them, so it must never be re-derived here (the bug that made
	// every deferred task fire with the callback prompt).
	for _, w := range wakesAll {
		n := w.Node
		// A recurring node re-arms at fire time, so it can become due again
		// while its own wake is still running; skip it until that wake clears,
		// coalescing the catch-up into one fire instead of overlapping turns.
		if s.wakeInflight(n.ID) {
			continue
		}
		// Hold a wake slot before disarming, mirroring the cmd path: a node we
		// return is always launched (and releases its slot). A full cap stops
		// collection so the drain loop returns instead of spinning.
		if !s.acquireWake() {
			break
		}
		if !s.disarm(ctx, &n, now) {
			s.releaseWake()
			continue
		}
		s.markInflight(n.ID)
		w.Node = n
		wakes = append(wakes, w)
	}
	return mech, conds, wakes
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

// acquireWake takes an agent-wake slot without blocking, reporting success.
func (s *Scheduler) acquireWake() bool {
	select {
	case s.wakeSem <- struct{}{}:
		return true
	default:
		return false
	}
}

func (s *Scheduler) releaseWake() { <-s.wakeSem }

func (s *Scheduler) wakeInflight(id NodeID) bool {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	_, ok := s.inflight[id]
	return ok
}

func (s *Scheduler) markInflight(id NodeID) {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	s.inflight[id] = struct{}{}
}

func (s *Scheduler) clearInflight(id NodeID) {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	delete(s.inflight, id)
}

// runMechanical discharges one claimed mechanical node — the kernel doing
// work with no model turn — and Kicks the loop so a successor fires
// immediately. It dispatches on the node's executor: shell runs a job, notify
// emits to the outbox. Both release the concurrency slot and the wait-group
// (Close waits within closeGrace).
func (s *Scheduler) runMechanical(n Node) {
	defer s.recoverFiring("mechanical", n.ID)
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
	// Speak into the chat that created the node; legacy nodes without an
	// origin fall back to the claiming owner (delivered as broadcast).
	target := n.Origin
	if target == "" {
		target = n.Owner
	}
	if err := s.notify(ctx, n.Notify, target, n.ID); err != nil {
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

// Fireable partitions the work the kernel can advance right now: mech are
// mechanical nodes (shell, notify) the kernel runs itself with no model turn
// (time-armed or not; equal-Seq groups launch together); conds are gated
// nodes whose shell predicate must be evaluated before their Do fires; wakes
// are triggered judgment nodes the kernel must wake a model for. A gated node
// is taken out before the mech/wake split because the gate, not the executor,
// decides whether it fires this period. Each wake's reason is classified here
// from the un-mutated fields — the one place it is decided, so a later disarm
// cannot invalidate it (a deferred timer is WakeDue; a callback whose gates
// cleared is WakeReady). wakes are ordered by due time then ID so firings
// reach the user in the order their intent sequenced them. Pure, so firing
// and classification are testable with a fake clock.
func Fireable(nodes []Node, now time.Time) (mech, conds []Node, wakes []WakeEvent) {
	for _, n := range nodes {
		if !Ready(n, GatedBySibling(nodes, n), now) {
			continue
		}
		if n.Gated() {
			conds = append(conds, n)
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
	return mech, conds, wakes
}

// dueAt is the instant a time-armed node became (or becomes) due.
func dueAt(n Node) time.Time {
	if n.Recurring() {
		return n.NextDue
	}
	return n.After
}
