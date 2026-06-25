package gateway

import (
	"strings"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// sampleTrajectory builds a tiny two-message session for rendering tests.
func sampleTrajectory() (core.Session, []core.Event) {
	now := time.Date(2026, 6, 25, 12, 0, 0, 0, time.UTC)
	sess := core.Session{ID: "sess-traj", Model: "gpt-5", CreatedAt: now}
	events := []core.Event{
		*core.NewMessageEvent("sess-traj", core.Message{Role: core.RoleUser, Content: "hello"}, now),
		*core.NewMessageEvent("sess-traj", core.Message{Role: core.RoleAssistant, Content: "hi there"}, now),
	}
	return sess, events
}

// TestTrajectoryAltLinksLeaveSandbox guards the share-page fix: the alternate-
// format links (event log JSONL, messages) must be navigable from the sandboxed
// (opaque-origin) trajectory page. A bare relative href like "?format=jsonl"
// does not resolve against the share URL from a null origin, so the links must
// carry the self path AND target the top frame so the navigation leaves the
// artifact sandbox.
func TestTrajectoryAltLinksLeaveSandbox(t *testing.T) {
	sess, events := sampleTrajectory()
	selfPath := "/s/sometoken"
	html := renderTrajectory(sess, events, selfPath)

	for _, format := range []string{"jsonl", "messages"} {
		wantHref := selfPath + "?format=" + format
		if !strings.Contains(html, `href="`+wantHref+`"`) {
			t.Errorf("trajectory HTML missing self-path link for format %q (want href=%q)", format, wantHref)
		}
	}
	// The links must break out of the sandbox frame, or a sandboxed
	// null-origin navigation is blocked / mis-resolved.
	if strings.Count(html, `target="_top"`) < 2 {
		t.Errorf("alternate-format links must use target=\"_top\" to leave the artifact sandbox; got HTML:\n%s", html)
	}
}
