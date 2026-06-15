package core_test

import (
	"strings"
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

func ev(seq int64, p core.EventPayload) core.Event {
	return core.Event{Seq: seq, Version: core.CurrentEventVersion, Payload: p}
}

func TestProjectMapsKindsToRoles(t *testing.T) {
	events := []core.Event{
		ev(0, core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "hi"}}),
		ev(1, core.MessagePayload{Message: core.Message{Role: core.RoleAssistant, Content: "yo"}}),
		ev(2, core.ToolResultPayload{Result: core.ToolResult{CallID: "c1", Content: "r"}}),
		ev(3, core.UsagePayload{Usage: core.Usage{TotalTokens: 9}}),
		ev(4, core.InterruptPayload{Reason: "x"}),
	}
	msgs := core.Project(events, "sys")

	// usage + interrupt are not part of the conversational projection.
	wantRoles := []core.Role{core.RoleSystem, core.RoleUser, core.RoleAssistant, core.RoleTool}
	if len(msgs) != len(wantRoles) {
		t.Fatalf("want %d messages, got %d (%+v)", len(wantRoles), len(msgs), msgs)
	}
	for i, r := range wantRoles {
		if msgs[i].Role != r {
			t.Fatalf("msg %d: want role %q got %q", i, r, msgs[i].Role)
		}
	}
	if msgs[3].ToolCallID != "c1" {
		t.Fatalf("tool message lost its call id: %q", msgs[3].ToolCallID)
	}
}

func TestProjectOmitsSystemWhenEmpty(t *testing.T) {
	msgs := core.Project([]core.Event{
		ev(0, core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "hi"}}),
	}, "")
	if len(msgs) != 1 || msgs[0].Role != core.RoleUser {
		t.Fatalf("unexpected projection: %+v", msgs)
	}
}

// TestProjectFoldsCompressedSpan is the correctness check for fold-in-place
// compression: events in a replaced span are dropped and the summary appears at
// the span's start, with recent turns intact.
func TestProjectFoldsCompressedSpan(t *testing.T) {
	events := []core.Event{
		ev(0, core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "old-1"}}),
		ev(1, core.MessagePayload{Message: core.Message{Role: core.RoleAssistant, Content: "old-2"}}),
		ev(2, core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "new-1"}}),
		ev(3, core.MessagePayload{Message: core.Message{Role: core.RoleAssistant, Content: "new-2"}}),
		ev(4, core.CompressionPayload{Summary: "SUMMARY", ReplacedSeqLo: 0, ReplacedSeqHi: 1}),
	}
	msgs := core.Project(events, "sys")

	// sys, SUMMARY (at span start), then the un-folded recent turns.
	want := []struct {
		role    core.Role
		content string
	}{
		{core.RoleSystem, "sys"},
		{core.RoleSystem, "SUMMARY"},
		{core.RoleUser, "new-1"},
		{core.RoleAssistant, "new-2"},
	}
	if len(msgs) != len(want) {
		t.Fatalf("want %d messages, got %d (%+v)", len(want), len(msgs), msgs)
	}
	for i, w := range want {
		if msgs[i].Role != w.role || msgs[i].Content != w.content {
			t.Fatalf("msg %d: want {%q,%q} got {%q,%q}", i, w.role, w.content, msgs[i].Role, msgs[i].Content)
		}
	}
}

// TestProjectRendersLatestContextPerSourceFenced checks the memory/injected
// context block: only the latest segment per source is rendered, fenced, as a
// trailing suffix AFTER the conversation — older injections stay in the log but
// are superseded. The suffix placement keeps the volatile block out of the
// cacheable conversation prefix.
func TestProjectRendersLatestContextPerSourceFenced(t *testing.T) {
	events := []core.Event{
		ev(0, core.ContextPayload{Segments: []core.Segment{{Source: "memory", Content: "old mem"}}}),
		ev(1, core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "hi"}}),
		ev(2, core.ContextPayload{Segments: []core.Segment{{Source: "memory", Content: "new mem"}}}),
	}
	msgs := core.Project(events, "sys")

	if len(msgs) != 3 {
		t.Fatalf("want 3 messages, got %d (%+v)", len(msgs), msgs)
	}
	if msgs[0].Role != core.RoleSystem || msgs[0].Content != "sys" {
		t.Fatalf("msg 0 should be the system prompt, got %+v", msgs[0])
	}
	if msgs[1].Role != core.RoleUser || msgs[1].Content != "hi" {
		t.Fatalf("conversation should precede the context suffix, got %+v", msgs[1])
	}
	block := msgs[2]
	if block.Role != core.RoleSystem {
		t.Fatalf("context block should be a system message, got role %q", block.Role)
	}
	if !strings.Contains(block.Content, "new mem") {
		t.Fatalf("context block should render the latest segment, got %q", block.Content)
	}
	if strings.Contains(block.Content, "old mem") {
		t.Fatalf("context block must supersede older segments, got %q", block.Content)
	}
	if !strings.Contains(block.Content, "<<memory>>") {
		t.Fatalf("context block should be fenced with provenance, got %q", block.Content)
	}

	// ProjectContext renders only the suffix block, byte-identical to Project's
	// trailing message — the engine composes it after the conversation prefix.
	ctxOnly := core.ProjectContext(events)
	if len(ctxOnly) != 1 || ctxOnly[0].Content != block.Content {
		t.Fatalf("ProjectContext should yield just the context block, got %+v", ctxOnly)
	}
}

// FuzzProject is the property seam: for any event sequence, Project never panics
// and every produced message carries a valid role. Only possible because
// Project is a pure core function.
func FuzzProject(f *testing.F) {
	f.Add([]byte{0, 1, 2, 3, 4, 5})
	f.Add([]byte{})
	f.Fuzz(func(t *testing.T, data []byte) {
		events := make([]core.Event, 0, len(data))
		for i, b := range data {
			var p core.EventPayload
			switch b % 6 {
			case 0:
				p = core.MessagePayload{Message: core.Message{Role: core.RoleUser, Content: "u"}}
			case 1:
				p = core.MessagePayload{Message: core.Message{Role: core.RoleAssistant, Content: "a"}}
			case 2:
				p = core.ToolResultPayload{Result: core.ToolResult{CallID: "c", Content: "r"}}
			case 3:
				p = core.UsagePayload{Usage: core.Usage{TotalTokens: 1}}
			case 4:
				p = core.CompressionPayload{Summary: "s", ReplacedSeqLo: 0, ReplacedSeqHi: int64(i)}
			default:
				p = core.ContextPayload{Segments: []core.Segment{{Source: "memory", Content: "m"}}}
			}
			events = append(events, ev(int64(i), p))
		}
		for _, m := range core.Project(events, "sys") {
			switch m.Role {
			case core.RoleSystem, core.RoleUser, core.RoleAssistant, core.RoleTool:
			default:
				t.Fatalf("projection produced invalid role %q", m.Role)
			}
		}
	})
}
