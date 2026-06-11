// Package engine is the kernel turn loop. It is the only place that knows how a
// turn flows (LLM -> tools -> repeat -> final); it knows nothing about
// transports, providers, or storage engines beyond the ports it depends on.
//
// Concurrency model: actor-per-session. Each session is owned by exactly one
// goroutine. Outside actors influence a session only by sending core.Intent on
// its inbound channel, and observe it only by reading core.KernelEvent on its
// outbound channel. There is no shared mutable session state and therefore no
// session-level locking — interrupts are context cancellation, not flags.
package engine

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/ports"
)

// Config tunes the engine. Defaults are applied by New.
type Config struct {
	Model         string
	SystemPrompt  string
	MaxIterations int
	// Reasoning is the requested reasoning effort for every turn; "" = provider
	// default. CacheRetention is the prompt-cache hint; "" = provider default.
	// Both flow onto each LLMRequest unchanged (the provider adapter maps them).
	Reasoning      core.ReasoningLevel
	CacheRetention core.CacheRetention
	// ExpandUser rewrites user-message text at projection time — the
	// slash-command hook. The log persists what the user typed (so replay,
	// titles, and edits see the raw command); only the provider-facing
	// conversation sees the expansion. nil = no rewriting.
	ExpandUser func(string) string
}

// Engine wires the ports together and spawns session actors. It is safe to
// share across sessions; per-session state lives in each actor goroutine.
type Engine struct {
	provider     ports.LLMProvider
	tools        ports.ToolRuntime
	store        ports.SessionStore
	clock        ports.Clock
	approval     ports.ApprovalPolicy // optional; nil = nothing requires approval
	ctxPol       ports.ContextPolicy  // optional; nil = never compress
	summ         ports.Summarizer     // optional; nil = marker-only summaries
	observer     ports.Observer       // optional; nil = no observation
	ctxInjectors []func(context.Context, core.SessionID) ([]core.Segment, error)
	onTurnEnd    func(context.Context, core.SessionID)
	cfg          Config

	// live indexes the currently-running session actors by id so an outside
	// caller can signal one without holding its Conversation — the seam the
	// browser door uses to interrupt a scheduler-spawned run it never started.
	// A session registers on StartSession and removes itself when its actor
	// exits; the actor model otherwise keeps per-session state lock-free, so
	// this map is the one shared structure and carries its own mutex.
	liveMu sync.Mutex
	live   map[core.SessionID]*Conversation
}

// Option configures optional engine dependencies without widening New's
// signature, so existing call sites keep compiling as capabilities are added.
type Option func(*Engine)

// WithApproval gates tool calls behind a human-confirmation policy. When unset,
// every tool call runs unattended (pi's default full-privileges behavior).
func WithApproval(p ports.ApprovalPolicy) Option { return func(e *Engine) { e.approval = p } }

// WithContextPolicy enables in-place context compression (ADR-0014): the policy
// decides when and how much to fold; the optional summarizer writes the summary
// (a nil summarizer yields a trivial marker). When the policy is unset, the
// conversation is never compressed.
func WithContextPolicy(p ports.ContextPolicy, s ports.Summarizer) Option {
	return func(e *Engine) { e.ctxPol = p; e.summ = s }
}

// WithObserver attaches an operational observer (structured logs/metrics/traces)
// fed by every emitted KernelEvent. When unset, the engine emits no telemetry.
func WithObserver(o ports.Observer) Option { return func(e *Engine) { e.observer = o } }

// WithContextInjector registers a context source whose segments are appended
// as a ContextPayload at the start of each turn, before the LLM call loop.
// Injectors accumulate: each source (memory recall, the background-job table,
// retrieval) registers its own and they run in registration order, their
// segments merged into one event. An injector that has nothing new to say
// returns no segments and appends nothing.
func WithContextInjector(fn func(context.Context, core.SessionID) ([]core.Segment, error)) Option {
	return func(e *Engine) { e.ctxInjectors = append(e.ctxInjectors, fn) }
}

// WithTurnEnd registers a hook invoked once after every turn that completed
// normally (not interrupted). It is the symmetric counterpart of the
// context injector — recall at turn start, curate at turn end — and the seam
// the agent's memory uses to fold the finished turn into durable atoms. The hook
// MUST be cheap and non-blocking (the session actor calls it inline); real work
// belongs on a background worker the hook merely enqueues to.
func WithTurnEnd(fn func(context.Context, core.SessionID)) Option {
	return func(e *Engine) { e.onTurnEnd = fn }
}

// awaitReg registers a turn's wait for a mid-turn response intent. The turn
// goroutine sends one to the actor (which owns c.intents) and blocks on resp;
// the actor routes the matching response back. The rendezvous is what keeps the
// pending-waiter map owned by a single goroutine — no shared mutable state.
type awaitReg struct {
	id   core.RequestID
	resp chan core.Intent
}

// reqCounter is a process-wide monotonic source of correlation ids for
// suspend-and-await requests. It only needs uniqueness, not session affinity.
var reqCounter atomic.Uint64

func nextRequestID() core.RequestID {
	return core.RequestID(fmt.Sprintf("req-%d", reqCounter.Add(1)))
}

func New(provider ports.LLMProvider, tools ports.ToolRuntime, store ports.SessionStore, clock ports.Clock, cfg Config, opts ...Option) *Engine {
	if cfg.MaxIterations <= 0 {
		cfg.MaxIterations = 50
	}
	if cfg.Model == "" {
		cfg.Model = "fake"
	}
	e := &Engine{provider: provider, tools: tools, store: store, clock: clock, cfg: cfg}
	for _, o := range opts {
		o(e)
	}
	return e
}

// Conversation is a handle to a running session actor. Callers Send intents and
// read Events; they never touch session internals directly. (Named distinctly
// from core.Session, which is the persisted metadata.)
type Conversation struct {
	id      core.SessionID
	intents chan core.Intent
	events  chan core.Envelope
	// awaits is an UNBUFFERED rendezvous: the turn goroutine registers a waiter
	// here and the actor receives it before the turn proceeds, so a response
	// intent can never arrive before its waiter is registered.
	awaits   chan awaitReg
	observer ports.Observer // optional operational observer; nil = none
	// model is a per-session override of the engine's configured model, set by a
	// SetModelIntent between turns. It is owned solely by the actor goroutine and
	// only mutated while idle (no turn running), so a running turn reads a stable
	// value without synchronization. Empty = use the engine's configured model.
	model string
}

func (c *Conversation) ID() core.SessionID           { return c.id }
func (c *Conversation) Send(i core.Intent)           { c.intents <- i }
func (c *Conversation) Events() <-chan core.Envelope { return c.events }

// SendCtx is Send that gives up when ctx is cancelled instead of blocking
// forever on a stalled actor. Reports whether the intent was delivered.
func (c *Conversation) SendCtx(ctx context.Context, i core.Intent) bool {
	select {
	case c.intents <- i:
		return true
	case <-ctx.Done():
		return false
	}
}

// Drive runs a started conversation to its own terminal event, forwarding
// every envelope to emit (nil to ignore) so a caller can relay nested
// activity. It is the one place the "send a prompt, drain to completion"
// loop lives — shared by every caller that spawns a session and waits for it
// (delegation, scheduler wakes), so they cannot drift.
//
// Termination keys on this conversation's own session: relayed descendant
// envelopes (a delegated child's events, Depth>0 with a different SessionID)
// are forwarded to emit but never end the drive — only this session's
// TurnComplete (returned), a non-retryable ErrorEvent (Go error), or an
// Interrupted (context.Canceled) does. A closed channel without a terminal
// event returns ctx.Err (the actor closes its events when ctx is cancelled).
func Drive(ctx context.Context, conv *Conversation, emit func(core.Envelope)) (core.TurnComplete, error) {
	for env := range conv.Events() {
		if emit != nil {
			emit(env)
		}
		if env.SessionID != conv.id {
			continue // relayed descendant activity; not this session's terminal
		}
		switch e := env.Event.(type) {
		case core.TurnComplete:
			return e, nil
		case core.ErrorEvent:
			// Any error ends the turn — the engine does not auto-retry, so a
			// retryable error is not followed by more events; waiting for one
			// would hang the drive until ctx cancels. Retryable is a hint for
			// an interactive frontend that can re-prompt; a spawned session
			// (delegation, wake) has no one to re-prompt it, so a failed run
			// returns its error to the caller.
			return core.TurnComplete{}, fmt.Errorf("session %s error (%s): %s", conv.id, e.Category, e.Err)
		case core.Interrupted:
			return core.TurnComplete{}, context.Canceled
		}
	}
	return core.TurnComplete{}, ctx.Err()
}

// TrySend delivers an intent without blocking, returning false if the actor's
// intent buffer is full. A frontend uses it for non-urgent intents so a flood
// cannot stall its frame reader (which would starve a later interrupt); urgent
// intents (interrupt) still use Send, which the actor drains promptly.
func (c *Conversation) TrySend(i core.Intent) bool {
	select {
	case c.intents <- i:
		return true
	default:
		return false
	}
}

// emit delivers a kernel event, wrapped in an Envelope tagged with this
// session's id (Depth 0 — relayed child events get a higher depth at the
// delegation layer, ADR-0013). It never outlives the turn: if ctx is cancelled
// (an interrupt) it abandons the send instead of blocking forever on a full
// events channel whose consumer has slowed or gone away. Without this, a blocked
// emit wedges the turn goroutine so cancellation can't preempt it — defeating
// the actor model's core promise. Returns false if the send was abandoned.
func (c *Conversation) emit(ctx context.Context, e core.KernelEvent) bool {
	return c.emitEnvelope(ctx, core.Envelope{SessionID: c.id, Depth: 0, Event: e})
}

// emitEnvelope delivers a pre-built envelope on the outbound channel. emit uses
// it for this session's own events (Depth 0); the live-relay sink uses it to
// forward a delegated child's events (the child's SessionID, depth incremented)
// so nested sub-agent activity streams to the frontend in real time. It is safe
// for concurrent callers (channel sends and the Observer are both safe), which
// is what lets a fan-out of parallel children relay into one stream.
func (c *Conversation) emitEnvelope(ctx context.Context, env core.Envelope) bool {
	// Observe first: the event happened regardless of whether delivery to the
	// (possibly slow or gone) frontend succeeds, and telemetry must not depend on
	// delivery. The observer contract requires this to be cheap and non-blocking.
	if c.observer != nil {
		c.observer.ObserveEvent(ctx, env.Event)
	}
	select {
	case <-ctx.Done():
		return false
	case c.events <- env:
		return true
	}
}

// SessionOption customizes a session at creation. Options apply only when the
// session is newly created (not on adoption/resume), since they set intrinsic
// creation-time metadata.
type SessionOption func(*core.Session)

// WithOrigin records what initiated a session (e.g. core.OriginScheduler), set
// atomically at creation so the row is never briefly mis-attributed and a
// later best-effort write can't silently leave it wrong.
func WithOrigin(origin string) SessionOption {
	return func(s *core.Session) { s.Origin = origin }
}

// WithOwner records the conversation a machine-spawned session serves and
// what spawned it (e.g. core.SpawnedByNode(12)); its voice and UI visibility
// route to that chat. Set at creation for the same atomicity reason as
// WithOrigin.
func WithOwner(owner core.SessionID, spawnedBy string) SessionOption {
	return func(s *core.Session) {
		s.Owner = owner
		s.SpawnedBy = spawnedBy
	}
}

// WithPrincipal records who owns/authorizes the session — the address of the
// agent's voice (core.PrincipalLocal on single-user hosts).
func WithPrincipal(principal string) SessionOption {
	return func(s *core.Session) { s.Principal = principal }
}

// StartSession creates (or, for a known id, adopts) a session and launches its
// actor goroutine, which runs until ctx is cancelled. The returned
// Conversation's Events channel is closed when the actor exits. SessionOptions
// apply only to a freshly created session.
func (e *Engine) StartSession(ctx context.Context, id core.SessionID, opts ...SessionOption) (*Conversation, error) {
	sess, err := e.store.Get(ctx, id)
	if err != nil {
		if !errors.Is(err, ports.ErrSessionNotFound) {
			return nil, fmt.Errorf("look up session %q: %w", id, err)
		}
		now := e.clock.Now()
		sess = core.Session{
			ID:        id,
			Status:    core.SessionActive,
			Model:     e.cfg.Model,
			CreatedAt: now,
			UpdatedAt: now,
		}
		for _, opt := range opts {
			opt(&sess)
		}
		// Every session has an addressable principal: the agent's voice
		// targets a person, not a conversation. Single-user hosts default to
		// the local principal; a multi-user door overrides via WithPrincipal.
		if sess.Principal == "" {
			sess.Principal = core.PrincipalLocal
		}
		if err := e.store.CreateSession(ctx, sess); err != nil {
			return nil, err
		}
	}
	c := &Conversation{
		id:       id,
		intents:  make(chan core.Intent, 8),
		events:   make(chan core.Envelope, 32),
		awaits:   make(chan awaitReg), // unbuffered rendezvous
		observer: e.observer,
		// Session.Model is the durable authority; the actor caches it so a
		// resumed session keeps the model a SetModelIntent switched it to.
		model: sess.Model,
	}
	e.registerConv(c)
	go e.runActor(ctx, c)
	return c, nil
}

// registerConv indexes a live conversation so Interrupt can find it.
func (e *Engine) registerConv(c *Conversation) {
	e.liveMu.Lock()
	defer e.liveMu.Unlock()
	if e.live == nil {
		e.live = make(map[core.SessionID]*Conversation)
	}
	e.live[c.id] = c
}

// unregisterConv drops a conversation from the live index, but only if it is
// still the one registered under that id — a resumed session may have replaced
// it, and the departing actor must not evict its successor.
func (e *Engine) unregisterConv(c *Conversation) {
	e.liveMu.Lock()
	defer e.liveMu.Unlock()
	if e.live[c.id] == c {
		delete(e.live, c.id)
	}
}

// Interrupt cancels the in-flight turn of a live session by id, reporting
// whether a live session was found to signal (a session that already finished
// returns false — there is nothing to stop). It is the external seam the
// browser door uses to stop a scheduler-spawned run: delivery is non-blocking
// so a stalled actor cannot wedge the caller, with a goroutine fallback for the
// rare full intent buffer so a momentary backlog does not drop the interrupt.
func (e *Engine) Interrupt(id core.SessionID) bool {
	e.liveMu.Lock()
	c := e.live[id]
	e.liveMu.Unlock()
	if c == nil {
		return false
	}
	if !c.TrySend(core.InterruptIntent{}) {
		go c.Send(core.InterruptIntent{})
	}
	return true
}

// runActor is the single owner of a session. It processes one intent at a time;
// while a prompt is in flight it watches for an interrupt and cancels the turn.
// Every intent variant is handled explicitly — no silent no-ops.
func (e *Engine) runActor(parent context.Context, c *Conversation) {
	defer close(c.events)
	defer e.unregisterConv(c)
	// turnSeq is owned solely by this goroutine (the session actor), so it needs
	// no synchronization — it is the per-session monotonic turn counter stamped
	// on every event a turn produces.
	var turnSeq int64
	// queue holds prompts that arrived while a turn was running. The actor is the
	// sole owner (no lock), and prompts run FIFO after the current turn — never
	// silently dropped. processPrompt returns any prompts it collected mid-turn.
	var queue []core.PromptIntent
	for {
		if len(queue) > 0 {
			next := queue[0]
			queue = queue[1:]
			turnSeq++
			queue = append(queue, e.processPrompt(parent, c, next.Text, next.Parts, turnSeq)...)
			continue
		}
		select {
		case <-parent.Done():
			return
		case intent, ok := <-c.intents:
			if !ok {
				return
			}
			switch it := intent.(type) {
			case core.PromptIntent:
				turnSeq++
				queue = append(queue, e.processPrompt(parent, c, it.Text, it.Parts, turnSeq)...)
			case core.SteerIntent:
				turnSeq++
				queue = append(queue, e.processPrompt(parent, c, it.Text, it.Parts, turnSeq)...)
			case core.InterruptIntent:
				// Idle: there is no in-flight turn to cancel.
			case core.ResumeIntent:
				// Resume is owned by session adoption (StartSession / the control
				// seam's "open" frame loads the existing log), so a ResumeIntent
				// reaching a live, already-adopted session is a no-op rather than
				// an error — emitting ErrInternal here turned a benign protocol
				// frame into a spurious failure for a frontend that sent it.
			case core.SetModelIntent:
				// Apply when idle: the actor owns c.model and no turn is running,
				// so subsequent turns use the new model with no shared-state race.
				// Persist first — Session.Model is the durable authority and a
				// resumed session must see the switch; only then update the cache.
				if err := e.persistModel(parent, c.id, it.Model); err != nil {
					c.emit(parent, core.ErrorEvent{Category: core.ErrPersist, Err: "persist model: " + err.Error()})
					continue
				}
				c.model = it.Model
			case core.CompactIntent:
				// Manual /compact: force a compaction pass now (when idle).
				e.compactNow(parent, c)
			case core.ApprovalResponseIntent, core.QuestionResponseIntent:
				// A response arriving while idle has no pending waiter — the turn
				// that asked already ended or was interrupted. Drop it benignly;
				// a late/duplicate frame is not an internal error.
			default:
				c.emit(parent, core.ErrorEvent{Category: core.ErrInternal, Err: "unhandled intent type"})
			}
		}
	}
}

// processPrompt runs a turn in a child context and races it against incoming
// interrupts. An InterruptIntent cancels the turn's context — the structural
// payoff of the actor model.
func (e *Engine) processPrompt(parent context.Context, c *Conversation, text string, parts []core.ContentBlock, turnID int64) []core.PromptIntent {
	// Attach turn correlation once, here, so every downstream append and (later)
	// log line/span joins on the same SessionID+TurnID. cctx is not cancellable
	// itself (only turnCtx is), so it is the right context for must-persist
	// writes like the interrupt marker.
	cctx := obs.With(parent, obs.Correlation{
		SessionID: string(c.id),
		TurnID:    turnID,
		TraceID:   obs.NewTraceID(),
	})
	turnCtx, cancel := context.WithCancel(cctx)
	defer cancel()

	done := make(chan struct{})
	// completedNormally records whether runTurn reached a terminal outcome
	// (TurnComplete / ErrorEvent / persist-failure) versus returning because the
	// turn was cancelled. The turn goroutine writes it before close(done), and
	// processPrompt only reads it after <-done, so the channel close provides the
	// happens-before — no lock, no race (verified under -race).
	var completedNormally bool
	go func() {
		defer close(done)
		// A panic in a real provider or tool must degrade this one turn, not
		// crash the host process (which would take down every other session's
		// actor with it).
		defer func() {
			if r := recover(); r != nil {
				c.emit(turnCtx, core.ErrorEvent{Category: core.ErrInternal, Err: fmt.Sprintf("panic in turn: %v", r)})
				completedNormally = true // a panic is a terminal outcome, not an interrupt
			}
		}()
		completedNormally = e.runTurn(turnCtx, c, text, parts)
	}()

	// queued collects prompts that arrive while this turn runs; the actor runs
	// them after this one (FIFO). Returning them — rather than dropping them —
	// is what makes "send a follow-up while it's working" safe.
	var queued []core.PromptIntent
	// pending maps a RequestID to the turn's waiter channel. It is owned solely
	// by this goroutine (the actor) — the turn only ever sends registrations and
	// reads its own resp channel — so it needs no synchronization.
	pending := map[core.RequestID]chan core.Intent{}
	for {
		select {
		case <-done:
			// A normally-completed turn is the single choke point every terminal
			// outcome (final answer, terminate, max-steps) flows through, so the
			// memory-curation hook fires here exactly once per turn. An interrupted
			// turn (completedNormally == false) is skipped; the next turn's curation
			// picks up the delta via the checkpoint, so nothing is lost.
			if completedNormally && e.onTurnEnd != nil {
				e.onTurnEnd(cctx, c.id)
			}
			return queued
		case <-parent.Done():
			cancel()
			<-done
			return queued
		case reg := <-c.awaits:
			// The turn paused for a response (approval, question). Register the
			// waiter; the matching response intent below routes back to it.
			pending[reg.id] = reg.resp
		case intent, ok := <-c.intents:
			if !ok {
				cancel()
				<-done
				return queued
			}
			switch it := intent.(type) {
			case core.InterruptIntent:
				cancel()
				<-done
				// H1: an interrupt can be dequeued at the same instant the turn
				// finishes on its own (select picks randomly among ready cases).
				// If the turn already reached a terminal outcome, this is a race,
				// not a real interrupt: honor the completion and do NOT write a
				// phantom InterruptPayload into the durable log or double-signal
				// the frontend (which already saw TurnComplete).
				if completedNormally {
					return queued
				}
				// Best-effort interrupt marker; the conversation log is already
				// durable, so a failed marker write must not change control flow.
				// cctx (not the cancelled turnCtx) carries the turn correlation.
				e.persistBestEffort(cctx, c, core.NewInterruptEvent(c.id, "user interrupt", e.clock.Now()))
				c.emit(parent, core.Interrupted{})
				return queued
			case core.SteerIntent:
				cancel()
				<-done
				// H1: if the turn finished on its own, honor the completion and
				// still run the steer text as the next turn (user intent).
				if completedNormally {
					return append(queued, core.PromptIntent{Text: it.Text, Parts: it.Parts})
				}
				// Silent cut: discard intra-turn queued prompts, no Interrupted.
				return []core.PromptIntent{{Text: it.Text, Parts: it.Parts}}
			case core.PromptIntent:
				// Don't drop it: queue it and tell the user it was kept.
				c.emit(parent, core.Queued{Text: it.Text})
				queued = append(queued, it)
			case core.ApprovalResponseIntent:
				routeResponse(c, pending, it.RequestID, it, parent)
			case core.QuestionResponseIntent:
				routeResponse(c, pending, it.RequestID, it, parent)
			case core.SetModelIntent, core.CompactIntent:
				// These apply only when idle (between turns). Ignoring them
				// mid-turn keeps the actor's single-writer and no-mid-turn-mutation
				// invariants intact; a frontend resends once the session is idle.
			default:
				// Any other intent mid-turn is a protocol error, surfaced not
				// swallowed.
				c.emit(parent, core.ErrorEvent{Category: core.ErrInternal, Err: "unexpected intent during active turn"})
			}
		}
	}
}

// routeResponse delivers a mid-turn response intent to its waiting turn. An
// answer with no matching pending request is a protocol error (e.g. a stale or
// duplicate response), surfaced rather than silently dropped.
func routeResponse(c *Conversation, pending map[core.RequestID]chan core.Intent, id core.RequestID, intent core.Intent, emitCtx context.Context) {
	w, ok := pending[id]
	if !ok {
		c.emit(emitCtx, core.ErrorEvent{Category: core.ErrInternal, Err: "response for unknown request " + string(id)})
		return
	}
	w <- intent
	delete(pending, id)
}

// project folds the log into the provider-facing conversation: core.Project,
// then the ExpandUser hook over user messages. Expansion lives here — at the
// read side, not the write side — so the persisted log keeps exactly what the
// user typed and every other consumer (replay, titles, edit-by-text) sees the
// raw message.
func (e *Engine) project(events []core.Event) []core.Message {
	msgs := core.Project(events, e.cfg.SystemPrompt)
	if f := e.cfg.ExpandUser; f != nil {
		for i := range msgs {
			if msgs[i].Role == core.RoleUser {
				msgs[i].Content = f(msgs[i].Content)
			}
		}
	}
	return msgs
}

// runTurn is the agentic loop. The event log is the source of truth: any append
// or history-load failure aborts the turn with an ErrorEvent rather than letting
// the in-memory turn diverge from the persisted log. The conversation
// projection is built once and extended incrementally to avoid re-reading the
// whole log every iteration.
//
// It returns true when the turn reached a terminal outcome (final answer,
// surfaced error, or persist failure) and false when it returned because the
// context was cancelled (an interrupt). processPrompt uses this to tell a real
// interrupt from one that merely raced a completing turn (H1).
func (e *Engine) runTurn(ctx context.Context, c *Conversation, userText string, userParts []core.ContentBlock) bool {
	// Attach the live-relay sink so a delegating tool can stream a child agent's
	// events into this session's outbound stream, incrementing Depth by one for
	// nested rendering and preserving the originating child SessionID. The depth
	// math lives here, in one place, so it composes to any nesting level
	// (grandchild D0 -> child relays D1 -> this session relays D2). Relayed
	// events are presentation only and never persisted. base is the stable,
	// cancellable turn context the sink checks before sending.
	base := ctx
	ctx = withRelay(ctx, func(env core.Envelope) {
		c.emitEnvelope(base, core.Envelope{SessionID: env.SessionID, Depth: env.Depth + 1, Event: env.Event})
	})
	// Attach the suspend-and-ask capability so a questioning tool (ask) can
	// pause the turn for the user's structured answer — the approval gate's
	// requestAndAwait, reached from inside a tool instead of before one.
	ctx = withAsker(ctx, func(title string, questions []core.Question) (core.QuestionResponseIntent, bool) {
		id := nextRequestID()
		intent, ok := e.requestAndAwait(base, c, id, core.QuestionRequest{RequestID: id, Title: title, Questions: questions})
		if !ok {
			return core.QuestionResponseIntent{}, false
		}
		qr, _ := intent.(core.QuestionResponseIntent)
		return qr, true
	})

	if !e.append(ctx, c, core.NewMessageEvent(c.id, core.Message{Role: core.RoleUser, Content: userText, Parts: userParts}, e.clock.Now())) {
		return true
	}

	var segs []core.Segment
	for _, inject := range e.ctxInjectors {
		s, err := inject(ctx, c.id)
		if err != nil {
			c.emit(ctx, core.ErrorEvent{Category: core.ErrInternal, Err: "context inject: " + err.Error()})
			return true
		}
		segs = append(segs, s...)
	}
	if len(segs) > 0 {
		if !e.append(ctx, c, core.NewContextEvent(c.id, segs, e.clock.Now())) {
			return true
		}
	}

	events, err := e.store.Events(ctx, c.id)
	if err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrHistory, Err: "load history: " + err.Error()})
		return true
	}
	msgs := e.project(events)

	// turnUsage aggregates token accounting across every LLM call in this turn,
	// so TurnComplete can report the turn's cost without the frontend re-reading
	// the per-response usage events from the log.
	var turnUsage core.Usage

	// Snapshot the toolset once per turn: it's fixed for the turn's duration,
	// and a byte-identical Tools block across iterations keeps the provider's
	// prompt-cache prefix stable (a changing tools block invalidates it).
	tools := e.tools.Schemas()

	for iter := 0; iter < e.cfg.MaxIterations; iter++ {
		if ctx.Err() != nil {
			return false
		}

		// Preflight compression: before spending an LLM call, fold the oldest
		// turns if the projected conversation is over budget. Compression is an
		// in-place log append (ADR-0014); on success we re-project so this turn
		// sees the compacted conversation. A persist failure here is turn-fatal,
		// same as any other source-of-truth write.
		if newMsgs, abort, did := e.maybeCompress(ctx, c, msgs); abort {
			return true
		} else if did {
			msgs = newMsgs
		}

		model := e.cfg.Model
		if c.model != "" {
			model = c.model
		}
		req := core.LLMRequest{
			Model:          model,
			Messages:       msgs,
			Tools:          tools,
			Stream:         true,
			Reasoning:      e.cfg.Reasoning,
			CacheRetention: e.cfg.CacheRetention,
			SessionID:      string(c.id),
		}
		chunks, err := e.provider.Stream(ctx, req)
		if err != nil {
			// A cancelled turn (interrupt/steer) aborts the in-flight request;
			// the transport surfaces that as an error, but it is not a provider
			// failure — report "cancelled", not a phantom ErrorEvent.
			if ctx.Err() != nil {
				return false
			}
			// Provider/transport failures are often transient (rate limit,
			// timeout); flag retryable so a frontend can offer "try again".
			c.emit(ctx, core.ErrorEvent{Category: core.ErrProvider, Retryable: true, Err: err.Error()})
			return true
		}

		sr := e.streamResponse(ctx, c, chunks)
		if ctx.Err() != nil {
			return false
		}
		// A mid-stream failure (truncated/aborted response) the adapter reported
		// must abort the turn — otherwise a partial response is mistaken for a
		// complete one. Retryable: re-issuing the same prompt may succeed.
		if sr.err != nil {
			c.emit(ctx, core.ErrorEvent{Category: core.ErrProvider, Retryable: true, Err: sr.err.Error()})
			return true
		}

		assistant := core.Message{
			Role:      core.RoleAssistant,
			Content:   sr.content,
			Reasoning: sr.reasoning,
			ToolCalls: sr.toolCalls,
		}
		assistantEv := core.NewMessageEvent(c.id, assistant, e.clock.Now())
		if !e.append(ctx, c, assistantEv) {
			return true
		}
		// Extend the in-turn conversation via the SAME core.ProjectEvent the
		// full-log Project uses, so a live turn and a replay can't diverge.
		if m, ok := core.ProjectEvent(*assistantEv); ok {
			msgs = append(msgs, m)
		}

		if sr.usage != nil {
			turnUsage.PromptTokens += sr.usage.PromptTokens
			turnUsage.CompletionTokens += sr.usage.CompletionTokens
			turnUsage.TotalTokens += sr.usage.TotalTokens
			if !e.append(ctx, c, core.NewUsageEvent(c.id, *sr.usage, e.clock.Now())) {
				return true
			}
		}

		if len(sr.toolCalls) == 0 {
			c.emit(ctx, core.TurnComplete{FinalResponse: sr.content, StopReason: stopReasonFor(sr.finishReason), Usage: turnUsage})
			return true
		}

		// Dispatch the requested tools. The scheduler partitions the batch by
		// per-call resource footprint and runs independent calls concurrently
		// (read-only calls, or writes to distinct files), serializing only
		// conflicting ones — so a mixed batch is no longer all-or-nothing. A
		// batch that needs human approval takes the sequential path (an approval
		// pause is inherently interactive). Either way results are appended in
		// call order, so the persisted Seq — and therefore replay — stays
		// deterministic regardless of execution order.
		var dr dispatchResult
		if e.canSchedule(sr.toolCalls) {
			dr = e.dispatchScheduled(ctx, c, sr.toolCalls)
		} else {
			dr = e.dispatchSequential(ctx, c, sr.toolCalls)
		}
		msgs = append(msgs, dr.msgs...)
		if dr.stop {
			return dr.completedNormally
		}
		// A batch whose results all asked to terminate ends the turn gracefully
		// without another LLM call (pi's terminate hint).
		if dr.terminate {
			c.emit(ctx, core.TurnComplete{StopReason: core.StopTerminated, Usage: turnUsage})
			return true
		}
	}

	// Hitting the iteration cap is a legible terminal state, not an error: the
	// turn produced real work and simply stopped at the guardrail. Surface it as
	// a completion with a distinct StopReason so the frontend shows partial
	// progress instead of a red error.
	c.emit(ctx, core.TurnComplete{StopReason: core.StopMaxSteps, Usage: turnUsage})
	return true
}

// append persists an event, emitting an ErrorEvent and signalling abort (false)
// on failure. The log is the source of truth; a dropped append must never be
// silent. This is the must-persist path; writes that happen while a turn is
// already tearing down go through persistBestEffort, which reports failures on
// the operational plane instead of aborting.
func (e *Engine) append(ctx context.Context, c *Conversation, ev *core.Event) bool {
	if err := e.store.AppendEvent(ctx, e.stamp(ctx, ev)); err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrPersist, Err: "persist " + string(ev.Payload.Kind()) + ": " + err.Error()})
		return false
	}
	return true
}

// persistBestEffort appends ev on a cancellation-free context, for writes that
// keep the log replayable while a turn is torn down (the interrupt marker, the
// backfill of undispatched tool_calls). The frontend may already be gone, so a
// failure cannot abort anything — but it is never silent: it surfaces through
// the observer, whose contract is cheap and non-blocking.
func (e *Engine) persistBestEffort(ctx context.Context, c *Conversation, ev *core.Event) {
	bg := context.WithoutCancel(ctx)
	if err := e.store.AppendEvent(bg, e.stamp(bg, ev)); err != nil && c.observer != nil {
		c.observer.ObserveEvent(bg, core.ErrorEvent{Category: core.ErrPersist, Err: "persist " + string(ev.Payload.Kind()) + ": " + err.Error()})
	}
}

// persistModel writes a model switch to the session record, the durable
// authority a resumed session reads at adoption.
func (e *Engine) persistModel(ctx context.Context, id core.SessionID, model string) error {
	sess, err := e.store.Get(ctx, id)
	if err != nil {
		return err
	}
	sess.Model = model
	sess.UpdatedAt = e.clock.Now()
	return e.store.UpdateSession(ctx, sess)
}

// EndSession marks a one-shot spawned session ended — the active -> ended
// lifecycle transition. A scheduler wake or a delegated child runs exactly one
// turn and is never resumed, so its session must not linger active forever
// (which would leave the activity panel spinning on a run that finished long
// ago). Interactive chats are NOT ended here: they persist across turns and
// stay active so they remain resumable. Already-ended sessions are a no-op.
func (e *Engine) EndSession(ctx context.Context, id core.SessionID) error {
	sess, err := e.store.Get(ctx, id)
	if err != nil {
		return err
	}
	if sess.Status == core.SessionEnded {
		return nil
	}
	sess.Status = core.SessionEnded
	sess.UpdatedAt = e.clock.Now()
	return e.store.UpdateSession(ctx, sess)
}

// stamp copies the turn correlation from ctx onto the event so every persisted
// event carries its TurnID. It is the single choke point for turn stamping;
// every persistence path routes through it. A missing correlation leaves TurnID
// zero (a non-engine write), which Validate permits by design.
func (e *Engine) stamp(ctx context.Context, ev *core.Event) *core.Event {
	if cor, ok := obs.From(ctx); ok {
		ev.TurnID = cor.TurnID
	}
	return ev
}
