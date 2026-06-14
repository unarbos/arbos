package obs

import (
	"context"
	"log/slog"
	"regexp"
)

const redactedMark = "[REDACTED]"

// secretPattern matches the credential shapes most likely to be accidentally
// logged. core.SecretValue already redacts itself everywhere it is
// marshaled/stringified; this is the second layer for raw strings that were
// never wrapped (ADR-0016/0017). It is best-effort defense-in-depth, not a
// guarantee — the primary control is that secrets stay SecretRefs until the
// broker boundary. Covers: bearer tokens, OpenAI keys, Slack tokens, AWS access
// key ids, GitHub tokens, Google API keys, and JWTs.
var secretPattern = regexp.MustCompile(`(?i)(` +
	`bearer\s+[a-z0-9._\-]+` +
	`|sk-[a-z0-9._\-]{8,}` +
	`|xox[baprs]-[a-z0-9\-]+` +
	`|AKIA[0-9A-Z]{16}` +
	`|gh[opsru]_[A-Za-z0-9]{30,}` +
	`|AIza[0-9A-Za-z._\-]{35}` +
	`|eyJ[A-Za-z0-9_\-]+\.eyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+` +
	`)`)

// RedactingHandler wraps a slog.Handler and scrubs secret-looking substrings
// from the message and every string attribute before they are written.
type RedactingHandler struct {
	inner slog.Handler
}

func NewRedactingHandler(inner slog.Handler) *RedactingHandler {
	return &RedactingHandler{inner: inner}
}

func (h *RedactingHandler) Enabled(ctx context.Context, level slog.Level) bool {
	return h.inner.Enabled(ctx, level)
}

func (h *RedactingHandler) Handle(ctx context.Context, r slog.Record) error {
	clean := slog.NewRecord(r.Time, r.Level, redactString(r.Message), r.PC)
	r.Attrs(func(a slog.Attr) bool {
		clean.AddAttrs(redactAttr(a))
		return true
	})
	return h.inner.Handle(ctx, clean)
}

func (h *RedactingHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	scrubbed := make([]slog.Attr, len(attrs))
	for i, a := range attrs {
		scrubbed[i] = redactAttr(a)
	}
	return &RedactingHandler{inner: h.inner.WithAttrs(scrubbed)}
}

func (h *RedactingHandler) WithGroup(name string) slog.Handler {
	return &RedactingHandler{inner: h.inner.WithGroup(name)}
}

func redactAttr(a slog.Attr) slog.Attr {
	switch a.Value.Kind() {
	case slog.KindString:
		return slog.String(a.Key, redactString(a.Value.String()))
	case slog.KindGroup:
		// Recurse so secrets nested inside a grouped attr are scrubbed too.
		group := a.Value.Group()
		scrubbed := make([]any, len(group))
		for i, g := range group {
			scrubbed[i] = redactAttr(g)
		}
		return slog.Group(a.Key, scrubbed...)
	case slog.KindAny:
		// A raw string logged via slog.Any bypasses KindString; scrub it.
		if s, ok := a.Value.Any().(string); ok {
			return slog.String(a.Key, redactString(s))
		}
		return a
	default:
		return a
	}
}

func redactString(s string) string {
	return secretPattern.ReplaceAllString(s, redactedMark)
}

// RedactString scrubs secret-looking substrings from s — the same best-effort
// pass the logging handler applies (see secretPattern). Exported for the share
// boundary: a trajectory link is the one path that ships log content outside
// the box, so it is scrubbed before leaving, while the local `arbos export`
// stays full-fidelity (your own data, your own machine).
func RedactString(s string) string {
	return redactString(s)
}
