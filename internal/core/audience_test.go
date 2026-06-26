package core

import "testing"

// TestDefaultAudienceMatchesADR pins the ADR-0041 D1 mapping: prose and the
// human-to-human side chat federate to the room; the agent's private trajectory
// (reasoning/usage/context/compaction) and bookkeeping stay local by default.
func TestDefaultAudienceMatchesADR(t *testing.T) {
	room := map[EventKind]bool{
		EventUserMessage:      true,
		EventAssistantMessage: true,
		EventChatNote:         true,
	}
	all := []EventKind{
		EventUserMessage, EventAssistantMessage, EventToolResult, EventUsage,
		EventCompressed, EventContext, EventInterrupted, EventConfig,
		EventChatNote, EventBranchAnchor,
	}
	for _, k := range all {
		want := AudienceLocal
		if room[k] {
			want = AudienceRoom
		}
		if got := DefaultAudience(k); got != want {
			t.Errorf("DefaultAudience(%q) = %d, want %d", k, got, want)
		}
	}
}

// TestAudienceZeroValueIsLocal guards the additive/backward-compatible property:
// an Event whose Audience was never set (including one decoded from a pre-Matrix
// log) is Local — it never federates. See ADR-0041 D1 / ADR-0010.
func TestAudienceZeroValueIsLocal(t *testing.T) {
	var e Event
	if e.Audience != AudienceLocal {
		t.Fatalf("zero-value Event.Audience = %d, want AudienceLocal (%d)", e.Audience, AudienceLocal)
	}
}
