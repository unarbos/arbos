package obs_test

import (
	"bytes"
	"context"
	"log/slog"
	"strings"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/obs"
)

func TestSlogObserverLogsCorrelatedToolEvent(t *testing.T) {
	var buf bytes.Buffer
	logger := slog.New(slog.NewTextHandler(&buf, &slog.HandlerOptions{Level: slog.LevelDebug}))
	o := obs.NewSlogObserver(logger)

	ctx := obs.With(context.Background(), obs.Correlation{SessionID: "s1", TurnID: 7, TraceID: "trace-x"})
	o.ObserveEvent(ctx, core.ToolStarted{Call: core.ToolCall{Name: "read_file"}})

	out := buf.String()
	for _, want := range []string{"tool started", "session_id=s1", "turn_id=7", "trace_id=trace-x", "tool=read_file"} {
		if !strings.Contains(out, want) {
			t.Fatalf("log line missing %q:\n%s", want, out)
		}
	}
}

func TestRedactingHandlerScrubsSecrets(t *testing.T) {
	var buf bytes.Buffer
	base := slog.NewTextHandler(&buf, &slog.HandlerOptions{Level: slog.LevelDebug})
	logger := slog.New(obs.NewRedactingHandler(base))

	logger.Info("auth attempt",
		slog.String("header", "Bearer sk-supersecretvalue12345"),
		slog.String("api_key", "sk-anothersecret9999"),
		slog.Int("status", 200),
	)

	out := buf.String()
	if strings.Contains(out, "supersecret") || strings.Contains(out, "anothersecret") {
		t.Fatalf("secret leaked into log: %s", out)
	}
	if !strings.Contains(out, "[REDACTED]") {
		t.Fatalf("expected redaction marker: %s", out)
	}
	if !strings.Contains(out, "status=200") {
		t.Fatalf("non-secret attrs must survive: %s", out)
	}
}

func TestRedactingHandlerScrubsMessage(t *testing.T) {
	var buf bytes.Buffer
	logger := slog.New(obs.NewRedactingHandler(slog.NewTextHandler(&buf, nil)))
	logger.Info("token is Bearer abcdef123456 ok")
	if strings.Contains(buf.String(), "abcdef123456") {
		t.Fatalf("secret in message not redacted: %s", buf.String())
	}
}

// TestRedactingHandlerCoversCommonCredentialShapes guards the broadened pattern
// (AWS / GitHub / Google / JWT), so a future log line carrying one of these
// can't leak.
func TestRedactingHandlerCoversCommonCredentialShapes(t *testing.T) {
	cases := map[string]string{
		"aws":    "AKIAIOSFODNN7EXAMPLE",
		"github": "ghp_1234567890abcdefghijklmnopqrstuvwxyz",
		"google": "AIzaSyA1234567890abcdefghijklmnopqrstuv",
		"jwt":    "eyJhbGciOi.eyJzdWIiOi.SflKxwRJSM",
	}
	for name, secret := range cases {
		t.Run(name, func(t *testing.T) {
			var buf bytes.Buffer
			logger := slog.New(obs.NewRedactingHandler(slog.NewTextHandler(&buf, nil)))
			logger.Info("creds", slog.String("v", secret))
			if strings.Contains(buf.String(), secret) {
				t.Fatalf("%s credential leaked: %s", name, buf.String())
			}
		})
	}
}
