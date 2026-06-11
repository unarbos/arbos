// Package gateway is the browser door onto the kernel: an HTTP server that
// carries the control seam over WebSocket (one connection = one control.Serve,
// frame-per-message) and serves the built web UI. It re-implements no turn
// logic — the browser speaks the exact protocol every other frontend speaks.
package gateway

import (
	"context"
	"encoding/json"
	"fmt"
	"io/fs"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/outbox"
	"github.com/unarbos/arbos/internal/plan"
	"github.com/unarbos/arbos/internal/sqlite"
)

// sessionListLimit caps the history picker; nobody scrolls past this.
const sessionListLimit = 200

// viaWeb is the outbox delivered_via marker for the browser door.
const viaWeb = "web"

// outboxPoll is how often the gateway checks for undelivered outbox messages
// while at least one browser is connected (mirrors the terminal's cadence).
const outboxPoll = 2 * time.Second

// Server hosts the web frontend's surface: the live seam, the session
// history endpoints, outbox delivery, and the static SPA.
type Server struct {
	Engine       *engine.Engine
	Store        *sqlite.Store // session history + outbox reads; nil disables both
	NewSessionID func() core.SessionID
	Drain        time.Duration // in-flight turn drain on disconnect; zero = control default
	Dist         fs.FS         // built SPA to serve at /; nil = API only
	// KillJob stops a background job by id (the UI's ✕ on a running job).
	// Wired by the host to the workspace's job table; nil disables the route.
	KillJob func(id string) error
	// FindJob resolves a background job by id for the terminal tab's tail
	// poll (status + journal). Wired by the host like KillJob; nil disables
	// the route.
	FindJob func(id string) (JobSnapshot, error)
	// Voice captures speech from the host machine's microphone and transcribes
	// it on the host (the composer's mic button). Wired by the host to the
	// local mic + transcriber; nil disables the voice routes.
	Voice VoiceRecorder
	// ModelsURL is the provider's model-catalog endpoint (e.g. OpenRouter's
	// public {base}/models). Empty leaves the picker with just the current
	// model and no catalog to filter.
	ModelsURL string
	// Model is the host's configured default model, returned as the active
	// selection so the composer can show what's running before a switch.
	Model string
	// Commands lists the available slash commands for the composer's popup.
	// Wired by the host to the agent's template loader; nil disables the route.
	Commands func() []CommandInfo
	// Root is the workspace the file routes resolve against (the same root
	// the session's tools use), so a surface the agent presented with show
	// is fetchable back by the panel that renders it. Empty disables the
	// file routes.
	Root string

	mu      sync.Mutex
	clients map[*wsLineWriter]bool

	// terms owns the interactive shells this gateway has spawned (the
	// browser's terminal tabs). Process-local on purpose: a shell is live
	// conversational state, not a durable artifact like a job.
	terms termHost
}

// VoiceRecorder captures speech from the host machine's microphone and
// transcribes it on the host. Start begins capture; Stop ends it and returns
// the recognized text. The recording spans the two calls (start, then stop),
// so an implementation must not tie its lifetime to a request context.
type VoiceRecorder interface {
	Start(ctx context.Context) error
	Stop(ctx context.Context) (string, error)
}

// register adds a live browser connection to the outbox fan-out.
func (s *Server) register(w *wsLineWriter) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.clients == nil {
		s.clients = make(map[*wsLineWriter]bool)
	}
	s.clients[w] = true
}

func (s *Server) unregister(w *wsLineWriter) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.clients, w)
}

func (s *Server) clientList() []*wsLineWriter {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]*wsLineWriter, 0, len(s.clients))
	for c := range s.clients {
		out = append(out, c)
	}
	return out
}

// noticeFrame is the gateway's own server frame for outbox delivery. It rides
// the same NDJSON stream as control frames; a client that doesn't know the
// type ignores it (the seam contract is forward-compatible by discrimination).
// Session names the chat the message belongs to ("" = broadcast-class), so a
// client can tell "this is mine — always render" from "ambient — render once,
// wherever the user is looking".
type noticeFrame struct {
	Type      string `json:"type"` // always "notice"
	Text      string `json:"text"`
	Session   string `json:"session,omitempty"`
	CreatedAt int64  `json:"created_at"` // unix milliseconds
}

// DeliverOutbox is the browser door on the outbox (the agent's voice between
// turns — scheduled firings, finished background work). Delivery is
// session-scoped: it claims only messages belonging to a currently-connected
// chat (plus broadcast-class messages) and writes each one to the connections
// bound to that chat alone — work created in one conversation never speaks
// into another. Messages for chats nobody has open stay unclaimed until that
// chat reconnects. Blocks until ctx is cancelled; run it as a goroutine next
// to the HTTP server.
func (s *Server) DeliverOutbox(ctx context.Context) {
	if s.Store == nil {
		return
	}
	tick := time.NewTicker(outboxPoll)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
		clients := s.clientList()
		if len(clients) == 0 {
			continue
		}
		bySession := make(map[string][]*wsLineWriter)
		sessions := make([]string, 0, len(clients))
		for _, c := range clients {
			sid := c.boundSession()
			if sid == "" {
				continue
			}
			if _, seen := bySession[sid]; !seen {
				sessions = append(sessions, sid)
			}
			bySession[sid] = append(bySession[sid], c)
		}
		msgs, err := s.Store.ClaimOutboxFor(ctx, viaWeb, core.PrincipalLocal, sessions, time.Now().Add(-outbox.StaleAfter))
		if err != nil || len(msgs) == 0 {
			continue
		}
		for _, m := range msgs {
			session := m.Session
			if outbox.IsBroadcast(session) {
				session = ""
			}
			b, err := json.Marshal(noticeFrame{
				Type:      "notice",
				Text:      m.Text,
				Session:   session,
				CreatedAt: m.CreatedAt.UnixMilli(),
			})
			if err != nil {
				continue
			}
			line := append(b, '\n')
			// Route to the owning chat's connections; broadcast-class and
			// stale-swept messages (whose chat has no connection) go to every
			// client, which renders them once in the visible tab.
			targets := bySession[m.Session]
			if len(targets) == 0 {
				targets = clients
			}
			for _, c := range targets {
				_, _ = c.Write(line)
			}
		}
	}
}

// Handler builds the route table.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/ws", s.handleWS)
	mux.HandleFunc("GET /api/models", s.handleModels)
	if s.Commands != nil {
		mux.HandleFunc("GET /api/commands", s.handleCommands)
	}
	if s.Store != nil {
		mux.HandleFunc("GET /api/sessions", s.handleSessions)
		mux.HandleFunc("GET /api/sessions/{id}/events", s.handleSessionEvents)
		mux.HandleFunc("GET /api/sessions/{id}/children", s.handleSessionChildren)
		mux.HandleFunc("GET /api/activity", s.handleActivity)
	}
	if s.KillJob != nil {
		mux.HandleFunc("POST /api/jobs/{id}/kill", sameOrigin(s.handleKillJob))
	}
	if s.FindJob != nil {
		// Same-origin even though it's a GET: it reads job journals (process
		// output), which a drive-by page must not be able to probe.
		mux.HandleFunc("GET /api/jobs/{id}/tail", sameOrigin(s.handleJobTail))
	}
	if s.Root != "" {
		mux.HandleFunc("POST /api/terminals", sameOrigin(s.handleTermCreate))
		mux.HandleFunc("DELETE /api/terminals/{id}", sameOrigin(s.handleTermClose))
		mux.HandleFunc("GET /api/terminals/{id}/ws", s.handleTermWS)
	}
	if s.Voice != nil {
		mux.HandleFunc("POST /api/voice/start", sameOrigin(s.handleVoiceStart))
		mux.HandleFunc("POST /api/voice/stop", sameOrigin(s.handleVoiceStop))
	}
	if s.Store != nil {
		mux.HandleFunc("POST /api/plan/{id}/cancel", sameOrigin(s.handleCancelPlanNode))
		mux.HandleFunc("POST /api/runs/{id}/stop", sameOrigin(s.handleStopRun))
	}
	if s.Root != "" {
		// Same-origin even though these are GETs: they read workspace files,
		// which a drive-by page must not be able to embed or probe.
		mux.HandleFunc("GET /api/file", sameOrigin(s.handleFile))
		mux.HandleFunc("PUT /api/file", sameOrigin(s.handleFileWrite))
		mux.HandleFunc("GET /raw/{path...}", sameOrigin(s.handleRaw))
	}
	if s.Dist != nil {
		mux.Handle("/", spaHandler(s.Dist))
	}
	return mux
}

// sameOrigin guards the mutating routes against cross-site requests: any web
// page can POST to localhost without a preflight, so a drive-by site could
// otherwise kill jobs in the user's arbos. Modern browsers stamp Sec-Fetch-Site
// on every request; "cross-site" is refused, anything else (same-origin, or
// absent for curl and other non-browser clients) passes.
func sameOrigin(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
			http.Error(w, "cross-site request refused", http.StatusForbidden)
			return
		}
		next(w, r)
	}
}

// jobIDRe matches the job ids the supervisor mints (j<seq> and journal names):
// a path-shaped id ("../escape") must never reach the jobs dir join.
var jobIDRe = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)

func (s *Server) handleKillJob(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !jobIDRe.MatchString(id) {
		http.Error(w, "bad job id", http.StatusBadRequest)
		return
	}
	if err := s.KillJob(id); err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleVoiceStart begins capturing speech from the host microphone (the
// composer's mic button, pressed). A capture already in flight is a conflict —
// there is one mic.
func (s *Server) handleVoiceStart(w http.ResponseWriter, r *http.Request) {
	if err := s.Voice.Start(r.Context()); err != nil {
		http.Error(w, err.Error(), http.StatusConflict)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleVoiceStop ends the capture and returns the transcribed text for the
// composer to drop in.
func (s *Server) handleVoiceStop(w http.ResponseWriter, r *http.Request) {
	text, err := s.Voice.Stop(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, map[string]any{"text": text})
}

// handleCancelPlanNode is the UI's ✕ on a scheduled task: it moves the node
// to cancelled (ending its recurrence) through the same compare-and-set the
// plan tool uses, so a session racing to claim the node loses cleanly. A node
// already terminal is a no-op success — the user's intent is satisfied.
func (s *Server) handleCancelPlanNode(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil {
		http.Error(w, "bad node id", http.StatusBadRequest)
		return
	}
	n, err := s.Store.PlanNode(r.Context(), plan.NodeID(id))
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	if plan.Terminal(n.Status) {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	ok, err := s.Store.SetPlanNodeStatusIf(
		r.Context(), n.ID, n.Status, plan.StatusCancelled, "cancelled by user", n.Owner)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if !ok {
		http.Error(w, "node changed state; retry", http.StatusConflict)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleStopRun is the ✕ on an agent-run row: it interrupts a live
// scheduler/delegate run by id, cancelling its in-flight turn through the same
// InterruptIntent a frontend's stop button sends. It refuses to touch a
// human-driven chat (only machine-spawned runs, identified by a non-empty
// Owner), so the browser door cannot interrupt arbitrary conversations. A run
// that already finished is a no-op success — the user's intent (it's stopped)
// is already satisfied. Stopping the run does not end its recurrence; the UI
// offers the schedule's own cancel for that.
func (s *Server) handleStopRun(w http.ResponseWriter, r *http.Request) {
	id := core.SessionID(r.PathValue("id"))
	sess, err := s.Store.Get(r.Context(), id)
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	if sess.Owner == "" {
		http.Error(w, "not a run", http.StatusBadRequest)
		return
	}
	s.Engine.Interrupt(id)
	w.WriteHeader(http.StatusNoContent)
}

// CommandInfo is one slash command the composer's popup can offer: the name
// the user types after "/", a one-line description, and an argument hint.
// Expansion stays server-side (the seam's expand hook); this is discovery only.
// Path is the template's source file (workspace-relative when under the root),
// so the popup's edit affordance can open it in a prompt-editor panel.
type CommandInfo struct {
	Name         string `json:"name"`
	Description  string `json:"description,omitempty"`
	ArgumentHint string `json:"argument_hint,omitempty"`
	Path         string `json:"path,omitempty"`
}

// handleCommands lists the slash commands for the composer's popup. The list
// is re-read per request so a freshly added prompt file shows up on the next
// popup open without restarting the host.
func (s *Server) handleCommands(w http.ResponseWriter, r *http.Request) {
	cmds := s.Commands()
	if cmds == nil {
		cmds = []CommandInfo{}
	}
	writeJSON(w, map[string]any{"commands": cmds})
}

// modelJSON is one selectable model from the provider's catalog — the fields
// the composer's picker renders (id to send via set_model, a friendly name to
// match against, the window for a hint).
type modelJSON struct {
	ID            string `json:"id"`
	Name          string `json:"name,omitempty"`
	ContextLength int    `json:"context_length,omitempty"`
}

// modelsClient bounds the catalog fetch so a slow provider can't wedge the
// picker; the listing is a small JSON blob, so the timeout is generous.
var modelsClient = &http.Client{Timeout: 10 * time.Second}

// handleModels proxies the provider's model catalog (OpenRouter's public
// /models list) so the composer's picker can offer every available model and
// the user can filter by typing. The browser talks only to the gateway, so
// this is also the seam that keeps the catalog same-origin. The current model
// rides along so the picker shows the active selection on open.
func (s *Server) handleModels(w http.ResponseWriter, r *http.Request) {
	out := []modelJSON{}
	if s.ModelsURL != "" {
		if list, err := fetchModelCatalog(r.Context(), s.ModelsURL); err == nil {
			out = list
		}
	}
	writeJSON(w, map[string]any{"models": out, "current": s.Model})
}

// fetchModelCatalog reads an OpenAI-compatible /models listing (the shape
// OpenRouter and the OpenAI API both return: {"data":[{id,name,...}]}).
func fetchModelCatalog(ctx context.Context, url string) ([]modelJSON, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := modelsClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("models catalog: %s", resp.Status)
	}
	var body struct {
		Data []struct {
			ID            string `json:"id"`
			Name          string `json:"name"`
			ContextLength int    `json:"context_length"`
		} `json:"data"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		return nil, err
	}
	out := make([]modelJSON, 0, len(body.Data))
	for _, m := range body.Data {
		out = append(out, modelJSON{ID: m.ID, Name: m.Name, ContextLength: m.ContextLength})
	}
	return out, nil
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

type sessionJSON struct {
	ID        string `json:"id"`
	Title     string `json:"title"`
	UpdatedAt int64  `json:"updated_at"` // unix milliseconds
}

// handleSessions lists resumable sessions for the history picker.
func (s *Server) handleSessions(w http.ResponseWriter, r *http.Request) {
	sums, err := s.Store.ListSessions(r.Context(), sessionListLimit)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	out := make([]sessionJSON, 0, len(sums))
	for _, sum := range sums {
		out = append(out, sessionJSON{
			ID:        string(sum.ID),
			Title:     sum.Title,
			UpdatedAt: sum.UpdatedAt.UnixMilli(),
		})
	}
	writeJSON(w, map[string]any{"sessions": out})
}

// childJSON is one scheduler-spawned run belonging to a chat. Node is the
// plan node that fired (0 for legacy wakes without one); Active reports a run
// still in flight, which tells the UI to poll its transcript.
type childJSON struct {
	ID        string `json:"id"`
	Node      int64  `json:"node,omitempty"`
	Active    bool   `json:"active"`
	CreatedAt int64  `json:"created_at"` // unix milliseconds
	UpdatedAt int64  `json:"updated_at"` // unix milliseconds
}

// handleSessionChildren lists the scheduled runs owned by one chat — the
// sub-agent tabs its UI offers. Scoping is the point: a run belongs to the
// conversation whose plan node spawned it, never to whichever chat is open.
func (s *Server) handleSessionChildren(w http.ResponseWriter, r *http.Request) {
	chat := r.PathValue("id")
	kids, err := s.Store.ScheduledChildren(r.Context(), chat)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	out := make([]childJSON, 0, len(kids))
	for _, c := range kids {
		node, _ := core.ParseSpawnedByNode(c.SpawnedBy)
		out = append(out, childJSON{
			ID:        string(c.ID),
			Node:      node,
			Active:    c.Status == core.SessionActive,
			CreatedAt: c.CreatedAt.UnixMilli(),
			UpdatedAt: c.UpdatedAt.UnixMilli(),
		})
	}
	writeJSON(w, map[string]any{"children": out})
}

const activityRunsLimit = 30

// standingJSON is one armed plan node — a standing obligation, wherever it
// was created.
type standingJSON struct {
	Node    int64  `json:"node"`
	Goal    string `json:"goal"`
	When    string `json:"when,omitempty"` // "every 1m" | "in 30m" | "on deps"
	Chat    string `json:"chat,omitempty"` // owning conversation; "" = legacy
	Status  string `json:"status"`
	Outcome string `json:"outcome,omitempty"` // last attempt's outcome
}

// activityRunJSON is one recent machine-spawned session, across all chats.
type activityRunJSON struct {
	ID        string `json:"id"`
	Chat      string `json:"chat"`
	Node      int64  `json:"node,omitempty"`
	Kind      string `json:"kind"` // "scheduled" | "delegate"
	Active    bool   `json:"active"`
	UpdatedAt int64  `json:"updated_at"` // unix milliseconds
	// Stale marks a scheduled run whose owning plan node is no longer a live
	// standing task (cancelled or otherwise terminal). The run is history — it
	// will never fire again — so the UI dims it instead of presenting it
	// alongside live recurrences as if the task were still ticking.
	Stale bool `json:"stale,omitempty"`
}

// handleActivity is the whole-organism view: every standing obligation in the
// global plan forest plus the latest autonomous runs across all chats. The
// per-chat UI scopes for focus; this is the one place the user sees what the
// agent is carrying overall — closing the gap where the agent (whose
// projection is global) knows more than any one conversation shows.
func (s *Server) handleActivity(w http.ResponseWriter, r *http.Request) {
	nodes, err := s.Store.OpenPlanNodes(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	attempts, err := s.Store.LastPlanAttempts(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	standing := make([]standingJSON, 0)
	// live is the set of node ids still standing (armed, non-terminal). A run
	// whose node is absent here belongs to a cancelled or finished task, so it
	// is history rather than an active obligation.
	live := make(map[int64]bool)
	for _, n := range nodes {
		if !n.Armed() || plan.Terminal(n.Status) {
			continue
		}
		live[int64(n.ID)] = true
		row := standingJSON{
			Node:   int64(n.ID),
			Goal:   n.Goal,
			When:   planWhen(n),
			Chat:   n.Origin,
			Status: string(n.Status),
		}
		if a, ok := attempts[n.ID]; ok {
			row.Outcome = a.Outcome
		}
		standing = append(standing, row)
	}

	owned, err := s.Store.RecentOwnedSessions(r.Context(), activityRunsLimit)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	runs := make([]activityRunJSON, 0, len(owned))
	for _, o := range owned {
		kind := "delegate"
		if o.Origin == core.OriginScheduler {
			kind = "scheduled"
		}
		node, _ := core.ParseSpawnedByNode(o.SpawnedBy)
		runs = append(runs, activityRunJSON{
			ID:        string(o.ID),
			Chat:      string(o.Owner),
			Node:      node,
			Kind:      kind,
			Active:    o.Status == core.SessionActive,
			UpdatedAt: o.UpdatedAt.UnixMilli(),
			// A scheduled run pointing at a node that is no longer standing is
			// from a cancelled/finished task — mark it so the UI shows history,
			// not a live recurrence. Delegated runs (no node) are never stale.
			Stale: node != 0 && !live[node],
		})
	}
	writeJSON(w, map[string]any{"standing": standing, "runs": runs})
}

// planWhen renders a node's trigger the way the chat UI does ("every 1m",
// "in 30m", "on deps").
func planWhen(n plan.Node) string {
	switch {
	case n.Recurring():
		return "every " + n.Every.String()
	case !n.After.IsZero():
		return "at " + n.After.Format("15:04")
	case n.WakeOnReady:
		return "on deps"
	default:
		return ""
	}
}

// replayJSON is one transcript-shaped event for seeding a resumed tab. It
// carries only what the UI renders — the projection/provider views stay
// server-side.
type replayJSON struct {
	Type      string              `json:"type"` // user | assistant | tool_result | interrupted
	Seq       int64               `json:"seq"`  // source event seq, the fork point for rewind/edit
	Text      string              `json:"text,omitempty"`
	Parts     []core.ContentBlock `json:"parts,omitempty"` // user-attached images, for re-render
	ToolCalls []core.ToolCall     `json:"tool_calls,omitempty"`
	CallID    string              `json:"call_id,omitempty"`
	Content   string              `json:"content,omitempty"`
	IsError   bool                `json:"is_error,omitempty"`
	Details   json.RawMessage     `json:"details,omitempty"`
}

// handleSessionEvents replays a session's visible history so a resumed tab
// renders the past transcript before live events stream in.
func (s *Server) handleSessionEvents(w http.ResponseWriter, r *http.Request) {
	id := core.SessionID(r.PathValue("id"))
	events, err := s.Store.Events(r.Context(), id)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	out := make([]replayJSON, 0, len(events))
	for _, ev := range events {
		switch p := ev.Payload.(type) {
		case core.MessagePayload:
			m := p.Message
			switch m.Role {
			case core.RoleUser:
				out = append(out, replayJSON{Type: "user", Seq: ev.Seq, Text: m.Content, Parts: m.Parts})
			case core.RoleAssistant:
				out = append(out, replayJSON{Type: "assistant", Seq: ev.Seq, Text: m.Content, ToolCalls: m.ToolCalls})
			}
		case core.ToolResultPayload:
			out = append(out, replayJSON{
				Type:    "tool_result",
				Seq:     ev.Seq,
				CallID:  p.Result.CallID,
				Content: p.Result.Content,
				IsError: p.Result.IsError,
				Details: p.Result.Details,
			})
		case core.InterruptPayload:
			out = append(out, replayJSON{Type: "interrupted", Seq: ev.Seq})
		}
	}
	writeJSON(w, map[string]any{"events": out})
}

// spaHandler serves the built SPA: real files as-is, everything else (client
// routes) falls back to index.html.
func spaHandler(dist fs.FS) http.Handler {
	fileServer := http.FileServerFS(dist)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := strings.TrimPrefix(r.URL.Path, "/")
		if p != "" {
			if f, err := dist.Open(p); err == nil {
				_ = f.Close()
				fileServer.ServeHTTP(w, r)
				return
			}
		}
		r2 := r.Clone(r.Context())
		r2.URL.Path = "/"
		fileServer.ServeHTTP(w, r2)
	})
}
