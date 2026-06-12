package piwire

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/secret"
)

// Cloud audio wiring: thin closures over OpenRouter's dedicated audio
// endpoints (/audio/transcriptions and /audio/speech), built from the same
// resolved Config and secret-broker discipline as the chat provider. The
// gateway holds only the closures — never the key — and the routes exist only
// when the configured base actually serves these endpoints (OpenRouter).
//
// STT is the composer's mic fallback for hosts without on-device dictation
// (Apple Speech is macOS-only) and the transcriber for voice-memo
// attachments; TTS speaks the agent's reply when the user enables it.

const (
	// defaultSTTModel favors the fast, cheap transcription tier; accuracy on
	// short composer dictation is indistinguishable from the larger models.
	defaultSTTModel = "openai/whisper-large-v3-turbo"
	// defaultTTSModel/Voice favor naturalness — completion summaries are a few
	// sentences, so the per-character premium over the budget models is noise.
	defaultTTSModel = "microsoft/mai-voice-2"
	defaultTTSVoice = "en-US-Harper:MAI-Voice-2"

	// audioTimeout bounds one audio round trip (transcribe a memo, synthesize
	// a summary) — generous for big voice memos, finite so the UI never hangs.
	audioTimeout = 120 * time.Second

	// speakMaxChars caps TTS input server-side: spoken summaries are a glance
	// substitute, not an audiobook, and the cap bounds per-call cost no matter
	// what a client sends.
	speakMaxChars = 1200
)

// AudioEndpoints reports whether the configured provider base serves the
// dedicated audio endpoints. They are OpenRouter's API surface, not part of
// the generic OpenAI-compatible contract, so the wiring keys on the host.
func (c Config) AudioEndpoints() bool {
	return c.openRouterHosted()
}

// openRouterHosted gates the surfaces beyond the OpenAI-compatible contract —
// audio, account credits — on the configured base actually being OpenRouter.
func (c Config) openRouterHosted() bool {
	if !c.HasLLM {
		return false
	}
	u, err := url.Parse(c.BaseURL)
	return err == nil && strings.HasSuffix(u.Hostname(), "openrouter.ai")
}

// audioClient is the per-process HTTP client + broker pair for the audio
// endpoints, mirroring the chat provider's redirect-refusing posture.
type audioClient struct {
	base   string
	broker *secret.Broker
	ref    core.SecretRef
	http   *http.Client
}

func (c Config) audioClient() *audioClient {
	host := ""
	if u, err := url.Parse(c.BaseURL); err == nil {
		host = u.Hostname()
	}
	ref := core.SecretRef{Name: c.KeyEnv}
	return &audioClient{
		base:   strings.TrimRight(c.BaseURL, "/"),
		broker: secret.NewBroker(c.keyProvider(), secret.Binding{Ref: ref, Hosts: []string{host}, Inject: secret.BearerInjector}),
		ref:    ref,
		http: &http.Client{
			Timeout:       audioTimeout,
			CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
		},
	}
}

func (a *audioClient) post(ctx context.Context, path string, body any) (*http.Response, error) {
	payload, err := json.Marshal(body)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, a.base+path, bytes.NewReader(payload))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if err := a.broker.Apply(ctx, a.ref, req); err != nil {
		return nil, err
	}
	resp, err := a.http.Do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != http.StatusOK {
		defer func() { _ = resp.Body.Close() }()
		snippet, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return nil, fmt.Errorf("%s: status %d: %s", path, resp.StatusCode, strings.TrimSpace(string(snippet)))
	}
	return resp, nil
}

// Transcriber returns the speech-to-text closure the gateway serves, or nil
// when the configured base has no audio endpoints. Audio arrives as base64
// (the endpoint's own wire shape, so the closure never re-encodes) plus the
// container format (webm, m4a, wav, …).
func (c Config) Transcriber() func(ctx context.Context, dataB64, format string) (string, error) {
	if !c.AudioEndpoints() {
		return nil
	}
	a := c.audioClient()
	model := envOr("ARBOS_STT_MODEL", defaultSTTModel)
	return func(ctx context.Context, dataB64, format string) (string, error) {
		resp, err := a.post(ctx, "/audio/transcriptions", map[string]any{
			"model":       model,
			"input_audio": map[string]string{"data": dataB64, "format": format},
		})
		if err != nil {
			return "", err
		}
		defer func() { _ = resp.Body.Close() }()
		var out struct {
			Text string `json:"text"`
		}
		if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
			return "", fmt.Errorf("decode transcription: %w", err)
		}
		return strings.TrimSpace(out.Text), nil
	}
}

// Speaker returns the text-to-speech closure the gateway serves, or nil when
// the configured base has no audio endpoints. The reader streams MP3 bytes.
func (c Config) Speaker() func(ctx context.Context, text string) (io.ReadCloser, error) {
	if !c.AudioEndpoints() {
		return nil
	}
	a := c.audioClient()
	model := envOr("ARBOS_TTS_MODEL", defaultTTSModel)
	voice := envOr("ARBOS_TTS_VOICE", defaultTTSVoice)
	return func(ctx context.Context, text string) (io.ReadCloser, error) {
		text = strings.TrimSpace(text)
		if text == "" {
			return nil, fmt.Errorf("speak: empty text")
		}
		if r := []rune(text); len(r) > speakMaxChars {
			text = string(r[:speakMaxChars])
		}
		resp, err := a.post(ctx, "/audio/speech", map[string]any{
			"model":           model,
			"input":           text,
			"voice":           voice,
			"response_format": "mp3",
		})
		if err != nil {
			return nil, err
		}
		return resp.Body, nil
	}
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
