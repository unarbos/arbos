package ports_test

import (
	"errors"
	"testing"

	"github.com/unarbos/arbos/internal/ports"
)

func TestProviderErrorRetryable(t *testing.T) {
	cases := []struct {
		status int
		want   bool
	}{
		{0, true},   // transport/network error: ephemeral by default
		{408, true}, // request timeout
		{425, true}, // too early
		{429, true}, // rate limit
		{500, true},
		{502, true},
		{503, true},
		{504, true},
		{529, true}, // anthropic overloaded
		{400, false},
		{401, false},
		{403, false},
		{404, false},
		{422, false},
	}
	for _, c := range cases {
		pe := &ports.ProviderError{StatusCode: c.status, Err: errors.New("x")}
		if got := pe.Retryable(); got != c.want {
			t.Errorf("status %d: Retryable() = %v, want %v", c.status, got, c.want)
		}
	}
}

func TestProviderErrorUnwrapAndMessage(t *testing.T) {
	inner := errors.New("boom")
	pe := &ports.ProviderError{StatusCode: 503, Err: inner}
	if pe.Error() != "boom" {
		t.Errorf("Error() = %q, want %q", pe.Error(), "boom")
	}
	if !errors.Is(pe, inner) {
		t.Error("errors.Is should unwrap to the inner error")
	}
	// errors.As must recover the typed error so the engine can classify it
	// after the adapter wraps it.
	wrapped := errors.Join(errors.New("context"), pe)
	var got *ports.ProviderError
	if !errors.As(wrapped, &got) || got.RetryAfter != pe.RetryAfter {
		t.Error("errors.As should recover the *ProviderError")
	}
}

func TestProviderErrorNilSafe(t *testing.T) {
	var pe *ports.ProviderError
	if pe.Retryable() {
		t.Error("nil ProviderError must not be retryable")
	}
}
