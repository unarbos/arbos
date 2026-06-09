// Package mind is the agent's memory: one global set of atoms (durable
// comprehensions) plus the two operations over it — Recall and Curate.
//
//	STREAM  the event log (what happened)            — owned by the engine
//	ATOMS   distilled notes to the agent's future self (what we know) — here
//	RECALL  atoms that match the present moment  → injected at turn start
//	CURATE  fold a finished turn's stream into atoms → run at turn end
//
// There is no workspace, session, or user scope: every session reads and writes
// the same atoms, so the agent learns once and remembers everywhere. Scope is
// emergent — an atom surfaces when it matches the present, wherever the agent
// runs. The intelligence is a single background LLM call (Curate); recall is a
// fast FTS lookup. Both are best-effort: a failure degrades memory for a turn,
// never breaks the turn.
package mind

import (
	"context"
	"io"
	"log/slog"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

const (
	recallLimit       = 12   // atoms considered for injection per turn
	recallBudgetChars = 4000 // cap on the injected recall block (~1k tokens)
	existingCap       = 100  // existing atoms shown to the curator for merging
	curateTimeout     = 90 * time.Second
	jobBuffer         = 64

	// maxDeltaEvents bounds how many events a single curation folds. A fresh
	// checkpoint on a long/resumed session would otherwise send the entire
	// history in one prompt — overflowing the context, burning tokens, and (since
	// the failed call never advances the checkpoint) re-running every turn
	// forever. We distill the most recent tail and still advance past the whole
	// delta, so curation stays bounded and never wedges.
	maxDeltaEvents = 60
	maxDeltaChars  = 12000 // ~3k tokens of rendered transcript per curation
)

// recallSource is the single fixed Segment source for all recalled atoms. Using
// one stable source means core.Project's latest-per-source rule replaces the
// whole recall block each turn instead of accumulating it — the prompt stays
// bounded no matter how many turns recall different atoms.
const recallSource = core.SourceMemory

// Store is the storage the mind needs: the global atom set, per-session curation
// checkpoints, and the session event log to curate from. *sqlite.Store satisfies
// it; the mind depends on this narrow interface, not a concrete DB.
type Store interface {
	RecallAtoms(ctx context.Context, match string, limit int) ([]core.Atom, error)
	AllAtoms(ctx context.Context, limit int) ([]core.Atom, error)
	AtomCheckpoint(ctx context.Context, sid core.SessionID) (int64, error)
	CommitCuration(ctx context.Context, sid core.SessionID, set []core.Atom, forget []string, newSeq int64) error
	Events(ctx context.Context, sid core.SessionID) ([]core.Event, error)
}

// Mind owns recall and a single background curation worker. It is safe to share
// across all session actors: recall is read-only, and curation funnels through
// one goroutine so atom writes never race.
type Mind struct {
	store Store
	llm   ports.LLMProvider
	model string
	log   *slog.Logger

	jobs chan core.SessionID
	// baseCtx is the parent of every curation context; cancelling it (via Close)
	// aborts an in-flight curation's LLM stream so shutdown is prompt.
	baseCtx context.Context
	cancel  context.CancelFunc
	// stop signals the worker to exit; done is closed when it has. jobs is never
	// closed (producers and the consumer race on shutdown), so there is no
	// send-on-closed panic to guard — Close uses stop/done instead.
	stop     chan struct{}
	done     chan struct{}
	stopOnce sync.Once
}

// New builds a Mind and starts its background curation worker. Call Close to
// stop the worker (production hosts run for the process lifetime and need not).
// A nil logger discards curation diagnostics — pass a real logger only on a sink
// that won't corrupt the frontend (never stderr under the TUI).
func New(store Store, llm ports.LLMProvider, model string, logger *slog.Logger) *Mind {
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(io.Discard, nil))
	}
	ctx, cancel := context.WithCancel(context.Background())
	m := &Mind{
		store:   store,
		llm:     llm,
		model:   model,
		log:     logger,
		jobs:    make(chan core.SessionID, jobBuffer),
		baseCtx: ctx,
		cancel:  cancel,
		stop:    make(chan struct{}),
		done:    make(chan struct{}),
	}
	go m.run()
	return m
}

// Close stops the background worker, cancels any in-flight curation, and waits
// for the worker to exit. It is idempotent and safe to call from multiple
// goroutines.
func (m *Mind) Close() {
	m.stopOnce.Do(func() {
		close(m.stop)
		m.cancel()
	})
	<-m.done
}

// Recall is the turn-start recall hook (engine.WithContextInjector). It reads the
// session's latest user message as the present cue, recalls matching atoms, and
// returns them as one fenced Segment. It never returns an error: memory is
// best-effort and must not abort a turn.
func (m *Mind) Recall(ctx context.Context, sid core.SessionID) ([]core.Segment, error) {
	events, err := m.store.Events(ctx, sid)
	if err != nil {
		return nil, nil
	}
	atoms, err := m.store.RecallAtoms(ctx, ftsQuery(recallCue(events)), recallLimit)
	if err != nil || len(atoms) == 0 {
		return nil, nil
	}
	content := renderAtoms(atoms, recallBudgetChars)
	if content == "" {
		return nil, nil
	}
	return []core.Segment{{Source: recallSource, Content: content}}, nil
}

// Curate is the turn-end curation hook (engine.WithTurnEnd). It enqueues the
// session for background curation without blocking the actor; if the worker is
// saturated the job is dropped (the next turn re-enqueues and the checkpoint
// ensures the delta is still picked up).
func (m *Mind) Curate(_ context.Context, sid core.SessionID) {
	select {
	case m.jobs <- sid:
	default:
	}
}

func (m *Mind) run() {
	defer close(m.done)
	for {
		select {
		case <-m.stop:
			return
		case sid := <-m.jobs:
			ctx, cancel := context.WithTimeout(m.baseCtx, curateTimeout)
			m.curateSession(ctx, sid)
			cancel()
		}
	}
}

// recallCue builds the present-moment text recall ranks against: the latest
// user message plus, when the session has been compacted, the latest checkpoint
// summary. Right after a fold the user's text is often a bare "continue" with
// no recallable keywords — the checkpoint's goal and next-steps text carries
// exactly the vocabulary of the folded work, so compaction's output feeds
// recall's input. ftsQuery caps the token count, and the user text comes first,
// so the present moment always dominates the cue.
func recallCue(events []core.Event) string {
	cue := lastUserText(events)
	for i := len(events) - 1; i >= 0; i-- {
		if p, ok := events[i].Payload.(core.CompressionPayload); ok {
			return cue + " " + p.Summary
		}
	}
	return cue
}

// lastUserText returns the content of the most recent user message — the present
// moment recall ranks against.
func lastUserText(events []core.Event) string {
	for i := len(events) - 1; i >= 0; i-- {
		if p, ok := events[i].Payload.(core.MessagePayload); ok && p.Message.Role == core.RoleUser {
			return p.Message.Content
		}
	}
	return ""
}

var wordRe = regexp.MustCompile(`[A-Za-z0-9_]{3,}`)

// ftsQuery turns free-form prompt text into a safe FTS5 MATCH expression: the
// distinct word tokens OR'd together. Returning "" makes RecallAtoms fall back
// to the most recent atoms.
func ftsQuery(present string) string {
	toks := wordRe.FindAllString(strings.ToLower(present), -1)
	if len(toks) == 0 {
		return ""
	}
	seen := map[string]bool{}
	var uniq []string
	for _, t := range toks {
		if seen[t] {
			continue
		}
		seen[t] = true
		uniq = append(uniq, t)
		if len(uniq) >= 24 {
			break
		}
	}
	return strings.Join(uniq, " OR ")
}

// renderAtoms formats atoms as a terse bullet list, stopping before the budget
// so the injected block stays bounded.
func renderAtoms(atoms []core.Atom, budgetChars int) string {
	var b strings.Builder
	for _, a := range atoms {
		line := "- " + strings.TrimSpace(a.Content) + "\n"
		if b.Len()+len(line) > budgetChars {
			break
		}
		b.WriteString(line)
	}
	return strings.TrimRight(b.String(), "\n")
}
