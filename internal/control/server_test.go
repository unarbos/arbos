package control_test

import (
	"bufio"
	"context"
	"encoding/json"
	"io"
	"path/filepath"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/control"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
	"github.com/unarbos/arbos/internal/sqlite"
)

type clientFrame struct {
	Type         string          `json:"type"`
	SessionID    core.SessionID  `json:"session_id,omitempty"`
	Intent       json.RawMessage `json:"intent,omitempty"`
	NewSessionID core.SessionID  `json:"new_session_id,omitempty"`
	AnchorSeq    int64           `json:"anchor_seq,omitempty"`
	AnchorStart  int             `json:"anchor_start,omitempty"`
	AnchorEnd    int             `json:"anchor_end,omitempty"`
	AnchorQuote  string          `json:"anchor_quote,omitempty"`
	Summary      string          `json:"summary,omitempty"`
}

type serverFrame struct {
	Type      string          `json:"type"`
	SessionID core.SessionID  `json:"session_id"`
	Envelope  json.RawMessage `json:"envelope"`
	Error     string          `json:"error"`
}

// harness wires a control server to a pair of pipes and gives the test a way to
// send client frames and read server frames.
type harness struct {
	toServer   *io.PipeWriter
	fromServer *bufio.Scanner
	enc        *json.Encoder
	done       chan error
}

func serve(t *testing.T, eng *engine.Engine) *harness {
	t.Helper()
	cr, sw := io.Pipe() // server reads cr; client writes sw
	sr, cw := io.Pipe() // client reads sr; server writes cw

	h := &harness{
		toServer:   sw,
		fromServer: bufio.NewScanner(sr),
		enc:        json.NewEncoder(sw),
		done:       make(chan error, 1),
	}
	id := 0
	go func() {
		h.done <- control.Serve(context.Background(), eng, cr, cw, func() core.SessionID {
			id++
			return core.SessionID("auto")
		}, 0)
	}()
	t.Cleanup(func() {
		_ = sw.Close()
		_ = cw.Close()
	})
	return h
}

func (h *harness) send(t *testing.T, f clientFrame) {
	t.Helper()
	if err := h.enc.Encode(f); err != nil {
		t.Fatalf("send: %v", err)
	}
}

func (h *harness) readUntil(t *testing.T, typ string) serverFrame {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for h.fromServer.Scan() {
		var f serverFrame
		if err := json.Unmarshal(h.fromServer.Bytes(), &f); err != nil {
			t.Fatalf("bad server frame: %v", err)
		}
		if f.Type == "error" {
			t.Fatalf("server error frame: %s", f.Error)
		}
		if f.Type == typ {
			return f
		}
		if time.Now().After(deadline) {
			break
		}
	}
	t.Fatalf("did not receive a %q frame", typ)
	return serverFrame{}
}

func encIntent(t *testing.T, i core.Intent) json.RawMessage {
	t.Helper()
	b, err := core.EncodeIntent(i)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestControlOpenPromptComplete(t *testing.T) {
	eng := engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	h := serve(t, eng)

	h.send(t, clientFrame{Type: "open"})
	opened := h.readUntil(t, "opened")
	if opened.SessionID == "" {
		t.Fatal("opened frame missing session id")
	}

	h.send(t, clientFrame{Type: "intent", Intent: encIntent(t, core.PromptIntent{Text: "hello"})})

	// Read event frames until the turn completes.
	for {
		ev := h.readUntil(t, "event")
		env, err := core.DecodeEnvelope(ev.Envelope)
		if err != nil {
			t.Fatal(err)
		}
		if tc, ok := env.Event.(core.TurnComplete); ok {
			if tc.FinalResponse == "" {
				t.Fatal("expected a final response over the seam")
			}
			return
		}
	}
}

// TestControlResumesPersistedSession proves resume across the seam: a session
// written via one engine is reopened by id through the control server (backed by
// the same SQLite file) and its history is intact.
func TestControlResumesPersistedSession(t *testing.T) {
	path := filepath.Join(t.TempDir(), "resume.db")
	store, err := sqlite.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()

	// First engine: run a turn so the session has history.
	eng1 := engine.New(fake.Provider{}, fake.Tools{}, store, fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	ctx := context.Background()
	conv, err := eng1.StartSession(ctx, "persist-me")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "first turn"})
	for env := range conv.Events() {
		if _, ok := env.Event.(core.TurnComplete); ok {
			break
		}
	}
	before, _ := store.Events(ctx, "persist-me")
	if len(before) == 0 {
		t.Fatal("expected persisted history after first turn")
	}

	// Second engine over the same store, reached through the control seam,
	// resumes the same id.
	eng2 := engine.New(fake.Provider{}, fake.Tools{}, store, fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	h := serve(t, eng2)
	h.send(t, clientFrame{Type: "open", SessionID: "persist-me"})
	opened := h.readUntil(t, "opened")
	if opened.SessionID != "persist-me" {
		t.Fatalf("resume opened the wrong session: %q", opened.SessionID)
	}

	h.send(t, clientFrame{Type: "intent", Intent: encIntent(t, core.PromptIntent{Text: "second turn"})})
	for {
		ev := h.readUntil(t, "event")
		env, _ := core.DecodeEnvelope(ev.Envelope)
		if _, ok := env.Event.(core.TurnComplete); ok {
			break
		}
	}

	// The resumed turn appended to the SAME log: history grew, not reset.
	after, _ := store.Events(ctx, "persist-me")
	if len(after) <= len(before) {
		t.Fatalf("resume did not extend the existing log: before=%d after=%d", len(before), len(after))
	}
}

// TestControlDrainOnEOF proves that closing stdin after a prompt still allows
// the in-flight turn to complete before Serve returns.
func TestControlDrainOnEOF(t *testing.T) {
	eng := engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})

	cr, sw := io.Pipe()
	sr, cw := io.Pipe()
	done := make(chan error, 1)
	go func() {
		done <- control.Serve(context.Background(), eng, cr, cw, func() core.SessionID {
			return "drain-test"
		}, 0)
	}()

	complete := make(chan struct{})
	opened := make(chan struct{})
	go func() {
		sc := bufio.NewScanner(sr)
		for sc.Scan() {
			var f serverFrame
			if json.Unmarshal(sc.Bytes(), &f) != nil {
				continue
			}
			switch f.Type {
			case "opened":
				close(opened)
			case "event":
				env, err := core.DecodeEnvelope(f.Envelope)
				if err != nil {
					continue
				}
				if _, ok := env.Event.(core.TurnComplete); ok {
					close(complete)
					return
				}
			}
		}
	}()

	enc := json.NewEncoder(sw)
	_ = enc.Encode(clientFrame{Type: "open"})
	select {
	case <-opened:
	case <-time.After(3 * time.Second):
		t.Fatal("no opened frame")
	}
	_ = enc.Encode(clientFrame{Type: "intent", Intent: encIntent(t, core.PromptIntent{Text: "hello"})})
	_ = sw.Close()

	select {
	case <-complete:
	case <-time.After(5 * time.Second):
		t.Fatal("turn did not complete")
	}

	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("serve returned error: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return after stdin EOF")
	}
	_ = cw.Close()
}

// TestControlBranchAcceptFlow exercises the discussion-branching frames over
// the seam: a parent session is opened and given a turn, a branch frame opens
// an anchored child (WITHOUT rebinding the connection — the parent stays the
// bound session), and accept_branch merges a curated summary back into the
// parent as a fenced context segment.
func TestControlBranchAcceptFlow(t *testing.T) {
	path := filepath.Join(t.TempDir(), "branch.db")
	store, err := sqlite.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()

	eng := engine.New(fake.Provider{}, fake.Tools{}, store, fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	h := serve(t, eng)

	h.send(t, clientFrame{Type: "open", SessionID: "parent"})
	h.readUntil(t, "opened")
	h.send(t, clientFrame{Type: "intent", Intent: encIntent(t, core.PromptIntent{Text: "explain"})})
	for {
		ev := h.readUntil(t, "event")
		env, err := core.DecodeEnvelope(ev.Envelope)
		if err != nil {
			t.Fatal(err)
		}
		if _, ok := env.Event.(core.TurnComplete); ok {
			break
		}
	}

	// Open a branch anchored at seq 0; the server must answer "branched" and
	// must NOT switch the bound session away from the parent.
	h.send(t, clientFrame{Type: "branch", NewSessionID: "child", AnchorSeq: 0, AnchorQuote: "explain"})
	br := h.readUntil(t, "branched")
	if br.SessionID != "child" {
		t.Fatalf("branched session = %q, want child", br.SessionID)
	}

	// The child exists, is parented to the parent, and carries the seeded
	// branch-anchor framing segment.
	ctx := context.Background()
	cs, err := store.Get(ctx, "child")
	if err != nil {
		t.Fatalf("get child: %v", err)
	}
	if cs.ParentID != "parent" {
		t.Errorf("child ParentID = %q, want parent", cs.ParentID)
	}
	pev, _ := store.Events(ctx, "parent")
	if a, ok := core.LatestBranchAnchor(pev, "child"); !ok || a.Status != core.BranchOpen {
		t.Errorf("parent anchor after branch = %+v ok=%v, want open", a, ok)
	}

	// Accept merges a curated summary back into the parent.
	h.send(t, clientFrame{Type: "accept_branch", NewSessionID: "child", Summary: "the agreed conclusion"})
	mg := h.readUntil(t, "merged")
	if mg.SessionID != "child" {
		t.Fatalf("merged session = %q, want child", mg.SessionID)
	}
	pev, _ = store.Events(ctx, "parent")
	if a, ok := core.LatestBranchAnchor(pev, "child"); !ok || a.Status != core.BranchAccepted {
		t.Errorf("parent anchor after accept = %+v ok=%v, want accepted", a, ok)
	}
	var fenced bool
	src := "<<" + core.BranchSegmentSource("child") + ">>"
	for _, m := range core.ProjectContext(pev) {
		if len(m.Content) >= len(src) && m.Content[:len(src)] == src {
			fenced = true
		}
	}
	if !fenced {
		t.Error("parent context has no fenced branch segment after accept")
	}
}

// TestControlBranchRefusesNoSession proves the branch frame errors cleanly when
// no session is bound, rather than panicking.
func TestControlBranchRefusesNoSession(t *testing.T) {
	eng := engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5})
	cr, sw := io.Pipe()
	sr, cw := io.Pipe()
	done := make(chan error, 1)
	go func() {
		done <- control.Serve(context.Background(), eng, cr, cw, func() core.SessionID { return "auto" }, 0)
	}()
	defer func() { _ = sw.Close(); _ = cw.Close() }()
	scan := bufio.NewScanner(sr)
	enc := json.NewEncoder(sw)
	_ = enc.Encode(clientFrame{Type: "branch", AnchorSeq: 0})
	for scan.Scan() {
		var f serverFrame
		if json.Unmarshal(scan.Bytes(), &f) == nil && f.Type == "error" {
			return
		}
	}
	t.Fatal("expected an error frame for branch with no session")
}
