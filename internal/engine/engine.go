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
}

// Engine wires the ports together and spawns session actors. It is safe to
// share across sessions; per-session state lives in each actor goroutine.
type Engine struct {
	provider  ports.LLMProvider
	tools     ports.ToolRuntime
	store     ports.SessionStore
	clock     ports.Clock
	approval  ports.ApprovalPolicy // optional; nil = nothing requires approval
	ctxPol    ports.ContextPolicy  // optional; nil = never compress
	summ      ports.Summarizer     // optional; nil = marker-only summaries
	observer  ports.Observer       // optional; nil = no observation
	ctxInject func(context.Context, core.SessionID) ([]core.Segment, error)
	cfg       Config
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

// WithContextInjector appends ContextPayload segments at the start of each turn,
// before the LLM call loop. Used for memory recall and similar injected context.
func WithContextInjector(fn func(context.Context, core.SessionID) ([]core.Segment, error)) Option {
	return func(e *Engine) { e.ctxInject = fn }
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
	// Observe first: the event happened regardless of whether delivery to the
	// (possibly slow or gone) frontend succeeds, and telemetry must not depend on
	// delivery. The observer contract requires this to be cheap and non-blocking.
	if c.observer != nil {
		c.observer.ObserveEvent(ctx, e)
	}
	select {
	case <-ctx.Done():
		return false
	case c.events <- core.Envelope{SessionID: c.id, Depth: 0, Event: e}:
		return true
	}
}

// StartSession creates (or, for a known id, adopts) a session and launches its
// actor goroutine, which runs until ctx is cancelled. The returned
// Conversation's Events channel is closed when the actor exits.
func (e *Engine) StartSession(ctx context.Context, id core.SessionID) (*Conversation, error) {
	if _, err := e.store.Get(ctx, id); err != nil {
		if !errors.Is(err, ports.ErrSessionNotFound) {
			return nil, fmt.Errorf("look up session %q: %w", id, err)
		}
		now := e.clock.Now()
		sess := core.Session{
			ID:        id,
			Status:    core.SessionActive,
			Model:     e.cfg.Model,
			CreatedAt: now,
			UpdatedAt: now,
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
	}
	go e.runActor(ctx, c)
	return c, nil
}

// runActor is the single owner of a session. It processes one intent at a time;
// while a prompt is in flight it watches for an interrupt and cancels the turn.
// Every intent variant is handled explicitly — no silent no-ops.
func (e *Engine) runActor(parent context.Context, c *Conversation) {
	defer close(c.events)
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
			queue = append(queue, e.processPrompt(parent, c, next.Text, turnSeq)...)
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
				queue = append(queue, e.processPrompt(parent, c, it.Text, turnSeq)...)
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
				c.model = it.Model
			case core.CompactIntent:
				// Manual /compact: force a compaction pass now (when idle).
				e.compactNow(parent, c)
			case core.ApprovalResponseIntent, core.ClarifyResponseIntent:
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
func (e *Engine) processPrompt(parent context.Context, c *Conversation, text string, turnID int64) []core.PromptIntent {
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
		completedNormally = e.runTurn(turnCtx, c, text)
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
			return queued
		case <-parent.Done():
			cancel()
			<-done
			return queued
		case reg := <-c.awaits:
			// The turn paused for a response (approval/clarify). Register the
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
				_ = e.store.AppendEvent(cctx, e.stamp(cctx, core.NewInterruptEvent(c.id, "user interrupt", e.clock.Now())))
				c.emit(parent, core.Interrupted{})
				return queued
			case core.PromptIntent:
				// Don't drop it: queue it and tell the user it was kept.
				c.emit(parent, core.Queued(it))
				queued = append(queued, it)
			case core.ApprovalResponseIntent:
				routeResponse(c, pending, it.RequestID, it, parent)
			case core.ClarifyResponseIntent:
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
func (e *Engine) runTurn(ctx context.Context, c *Conversation, userText string) bool {
	if !e.append(ctx, c, core.NewMessageEvent(c.id, core.Message{Role: core.RoleUser, Content: userText}, e.clock.Now())) {
		return true
	}

	if e.ctxInject != nil {
		segs, err := e.ctxInject(ctx, c.id)
		if err != nil {
			c.emit(ctx, core.ErrorEvent{Category: core.ErrInternal, Err: "context inject: " + err.Error()})
			return true
		}
		if len(segs) > 0 {
			if !e.append(ctx, c, core.NewContextEvent(c.id, segs, e.clock.Now())) {
				return true
			}
		}
	}

	events, err := e.store.Events(ctx, c.id)
	if err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrHistory, Err: "load history: " + err.Error()})
		return true
	}
	msgs := core.Project(events, e.cfg.SystemPrompt)

	// turnUsage aggregates token accounting across every LLM call in this turn,
	// so TurnComplete can report the turn's cost without the frontend re-reading
	// the per-response usage events from the log.
	var turnUsage core.Usage

	// readOnly maps tool name -> parallel-safe. Built once per turn from the
	// runtime's advertised schemas; the engine never parses tool args, so this
	// coarse classification is the whole basis for the parallel-dispatch
	// decision below.
	readOnly := make(map[string]bool)
	for _, sc := range e.tools.Schemas() {
		readOnly[sc.Name] = sc.ReadOnly
	}

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
			Tools:          e.tools.Schemas(),
			Stream:         true,
			Reasoning:      e.cfg.Reasoning,
			CacheRetention: e.cfg.CacheRetention,
			SessionID:      string(c.id),
		}
		chunks, err := e.provider.Stream(ctx, req)
		if err != nil {
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
			e.mirrorTokenCount(ctx, c.id, sr.usage.TotalTokens)
		}

		if len(sr.toolCalls) == 0 {
			c.emit(ctx, core.TurnComplete{FinalResponse: sr.content, StopReason: core.StopAnswered, Usage: turnUsage})
			return true
		}

		// Dispatch the requested tools. A batch of exclusively read-only tools
		// runs concurrently (each tool's cost is its I/O); anything that writes,
		// or a mixed batch, runs sequentially to avoid ordering/same-resource
		// hazards. Either way results are appended in call order, so the
		// persisted Seq — and therefore replay — stays deterministic.
		var dr dispatchResult
		if e.canParallelize(sr.toolCalls, readOnly) {
			dr = e.dispatchParallel(ctx, c, sr.toolCalls)
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
// silent.
func (e *Engine) append(ctx context.Context, c *Conversation, ev *core.Event) bool {
	if err := e.store.AppendEvent(ctx, e.stamp(ctx, ev)); err != nil {
		c.emit(ctx, core.ErrorEvent{Category: core.ErrPersist, Err: "persist " + string(ev.Payload.Kind()) + ": " + err.Error()})
		return false
	}
	return true
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

// mirrorTokenCount updates the derived Session.TokenCount. The authoritative
// record is the usage event already in the log, so a transient metadata-write
// failure is non-fatal and does not abort the turn.
func (e *Engine) mirrorTokenCount(ctx context.Context, id core.SessionID, addTokens int) {
	sess, err := e.store.Get(ctx, id)
	if err != nil {
		return
	}
	sess.TokenCount += addTokens
	sess.UpdatedAt = e.clock.Now()
	_ = e.store.UpdateSession(ctx, sess)
}
