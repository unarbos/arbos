package core

import (
	"testing"
	"time"
)

// A BranchAnchorPayload round-trips through the kind-discriminated payload codec
// (which is exhaustive-linted, so a missing case fails the build).
func TestBranchAnchorPayloadRoundTrip(t *testing.T) {
	p := BranchAnchorPayload{
		Branch:  "child-1",
		Seq:     7,
		Start:   3,
		End:     19,
		Quote:   "the highlighted bit",
		Status:  BranchAccepted,
		Summary: "we agreed to keep it",
	}
	b, err := EncodePayload(p)
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	got, err := DecodePayload(EventBranchAnchor, b)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	ap, ok := got.(BranchAnchorPayload)
	if !ok {
		t.Fatalf("decoded to %T, want BranchAnchorPayload", got)
	}
	if ap != p {
		t.Errorf("round-trip mismatch: %+v vs %+v", ap, p)
	}
}

// A branch anchor MUST NOT project to a model message: the model never sees the
// anchor bookkeeping; only the accepted summary reaches it (as a context segment).
func TestBranchAnchorNotProjected(t *testing.T) {
	ev := NewBranchAnchorEvent("parent", BranchAnchorPayload{
		Branch: "child", Seq: 2, Quote: "x", Status: BranchOpen,
	}, time.Unix(0, 0))
	if _, ok := ProjectEvent(*ev); ok {
		t.Error("ProjectEvent returned a message for a branch anchor — it would leak bookkeeping into the model context")
	}
	msgs := ProjectConversation([]Event{*ev})
	if len(msgs) != 0 {
		t.Errorf("ProjectConversation yielded %d messages for a branch-anchor-only log, want 0", len(msgs))
	}
}

// The branch anchor constructor stamps the schema version and validates.
func TestBranchAnchorEventValidates(t *testing.T) {
	ev := NewBranchAnchorEvent("parent", BranchAnchorPayload{Branch: "c", Seq: 1, Status: BranchOpen}, time.Unix(0, 0))
	if ev.Version != CurrentEventVersion {
		t.Errorf("version = %d, want %d", ev.Version, CurrentEventVersion)
	}
	if err := ev.Validate(); err != nil {
		t.Errorf("Validate rejected a branch anchor: %v", err)
	}
}

// LatestBranchAnchor returns the most recent payload for a given branch, so a
// branch's status reads open -> accepted as later payloads supersede earlier.
func TestLatestBranchAnchor(t *testing.T) {
	now := time.Unix(0, 0)
	events := []Event{
		*NewBranchAnchorEvent("p", BranchAnchorPayload{Branch: "a", Seq: 1, Status: BranchOpen, Quote: "qa"}, now),
		*NewBranchAnchorEvent("p", BranchAnchorPayload{Branch: "b", Seq: 2, Status: BranchOpen, Quote: "qb"}, now),
		*NewBranchAnchorEvent("p", BranchAnchorPayload{Branch: "a", Seq: 1, Status: BranchAccepted, Quote: "qa", Summary: "done"}, now),
	}
	got, ok := LatestBranchAnchor(events, "a")
	if !ok {
		t.Fatal("LatestBranchAnchor(a) not found")
	}
	if got.Status != BranchAccepted || got.Summary != "done" {
		t.Errorf("latest for a = %+v, want accepted/done", got)
	}
	gotB, ok := LatestBranchAnchor(events, "b")
	if !ok || gotB.Status != BranchOpen {
		t.Errorf("latest for b = %+v, %v, want open", gotB, ok)
	}
	if _, ok := LatestBranchAnchor(events, "missing"); ok {
		t.Error("LatestBranchAnchor(missing) reported found")
	}
}

// An accepted branch's summary, injected as a ContextPayload segment under its
// branch source, projects as a fenced block — the merge-back mechanic — and a
// re-accept (same source) supersedes rather than accumulates.
func TestBranchSegmentProjectsFencedAndSupersedes(t *testing.T) {
	now := time.Unix(0, 0)
	src := BranchSegmentSource("child")
	events := []Event{
		*NewContextEvent("p", []Segment{{Source: src, Content: "first conclusion"}}, now),
		*NewContextEvent("p", []Segment{{Source: src, Content: "revised conclusion"}}, now),
	}
	msgs := ProjectContext(events)
	if len(msgs) != 1 {
		t.Fatalf("got %d context messages, want 1 (latest-per-source)", len(msgs))
	}
	want := "<<branch:child>>\nrevised conclusion\n<</branch:child>>"
	if msgs[0].Content != want {
		t.Errorf("fenced segment =\n%q\nwant\n%q", msgs[0].Content, want)
	}
}
