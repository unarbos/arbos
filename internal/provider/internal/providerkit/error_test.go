package providerkit

import (
	"errors"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/ports"
)

func TestParseRetryAfter(t *testing.T) {
	cases := []struct {
		val  string
		want time.Duration
	}{
		{"", 0},
		{"5", 5 * time.Second},
		{"0", 0},
		{"-3", 0},
		{"  12 ", 12 * time.Second},
		{"Wed, 21 Oct 2025 07:28:00 GMT", 0}, // HTTP-date form not parsed; falls back to backoff
		{"garbage", 0},
	}
	for _, c := range cases {
		h := http.Header{}
		if c.val != "" {
			h.Set("Retry-After", c.val)
		}
		if got := ParseRetryAfter(h); got != c.want {
			t.Errorf("Retry-After %q: got %v, want %v", c.val, got, c.want)
		}
	}
}

func TestNonOKErrorClassifies(t *testing.T) {
	resp := &http.Response{
		StatusCode: http.StatusTooManyRequests,
		Header:     http.Header{"Retry-After": []string{"7"}},
		Body:       io.NopCloser(strings.NewReader("rate limited")),
	}
	err := NonOKError("openai", resp)
	var pe *ports.ProviderError
	if !errors.As(err, &pe) {
		t.Fatalf("NonOKError should return *ports.ProviderError, got %T", err)
	}
	if pe.StatusCode != 429 {
		t.Errorf("StatusCode = %d, want 429", pe.StatusCode)
	}
	if !pe.Retryable() {
		t.Error("429 should be retryable")
	}
	if pe.RetryAfter != 7*time.Second {
		t.Errorf("RetryAfter = %v, want 7s", pe.RetryAfter)
	}
}

func TestTransportErrorIsRetryable(t *testing.T) {
	err := TransportError("anthropic", errors.New("connection reset"))
	var pe *ports.ProviderError
	if !errors.As(err, &pe) {
		t.Fatalf("TransportError should return *ports.ProviderError, got %T", err)
	}
	if pe.StatusCode != 0 || !pe.Retryable() {
		t.Errorf("transport error should have status 0 and be retryable, got status %d retryable %v", pe.StatusCode, pe.Retryable())
	}
}
