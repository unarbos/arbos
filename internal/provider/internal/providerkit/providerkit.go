// Package providerkit holds the machinery shared by every ports.LLMProvider
// adapter (openai, anthropic, google): the SSE read loop, channel send with
// ctx, an error-snippet reader, the streamed tool-call accumulator, and the
// Base struct + options for the duplicated Provider plumbing (name, http client
// with redirect refusal, auth). Each adapter then shrinks to just its wire
// translation. It lives under provider/internal so only the adapters can import
// it.
package providerkit

import (
	"bufio"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Authorizer attaches credentials to an outbound request (the secret broker in
// production; a no-op in tests). Every adapter shares this contract; keeping it
// an interface means an adapter depends on the capability, not the secret
// package.
type Authorizer interface {
	Apply(ctx context.Context, ref core.SecretRef, req *http.Request) error
}

// ImageOmittedPlaceholder stands in for an image on a wire channel that is
// text-only (every provider's tool/function-result channel). Emitting real image
// content there is a hard provider error (OpenAI 400s) and would permanently
// wedge a session, since the bad tool result re-projects every later turn.
const ImageOmittedPlaceholder = "[image omitted]"

// ToolResultText renders a tool-role message as text for the tool/result wire
// channel: the text content followed by one placeholder per image part. The
// model still learns an image was produced, but no provider sees invalid image
// content on a tool role.
func ToolResultText(m core.Message) string {
	segs := make([]string, 0, len(m.Parts)+1)
	if m.Content != "" {
		segs = append(segs, m.Content)
	}
	for _, p := range m.Parts {
		switch p.Type {
		case core.BlockText:
			if p.Text != "" {
				segs = append(segs, p.Text)
			}
		case core.BlockImage:
			segs = append(segs, ImageOmittedPlaceholder)
		}
	}
	return strings.Join(segs, "\n")
}

// Base is the per-provider plumbing every adapter embeds: the display name, the
// HTTP client, the credential authorizer + ref, and advertised capabilities.
type Base struct {
	Name       string
	HTTPClient *http.Client
	Auth       Authorizer
	SecretRef  core.SecretRef
	Caps       ports.Capabilities
}

// Option configures a Base.
type Option func(*Base)

// WithName overrides the provider name.
func WithName(n string) Option { return func(b *Base) { b.Name = n } }

// WithHTTPClient injects an http.Client (timeouts, transport, test servers).
func WithHTTPClient(c *http.Client) Option { return func(b *Base) { b.HTTPClient = c } }

// WithAuth wires a credential authorizer (the secret broker) and the ref to
// attach. Without it, requests go out unauthenticated.
func WithAuth(a Authorizer, ref core.SecretRef) Option {
	return func(b *Base) { b.Auth = a; b.SecretRef = ref }
}

// WithCapabilities advertises provider capabilities.
func WithCapabilities(c ports.Capabilities) Option { return func(b *Base) { b.Caps = c } }

// DefaultHTTPTimeout bounds a single provider request (connect + full body read).
// Without it a stalled upstream TCP read wedges the session actor until the OS
// gives up; interrupts help only when a client sends them.
const DefaultHTTPTimeout = 10 * time.Minute

// NewBase builds a Base with the default name and capabilities, then applies
// opts. The default client refuses to FOLLOW redirects: Go strips the
// Authorization header on a cross-host redirect but not custom headers (an
// x-api-key style secret), so a malicious endpoint could 30x a request to
// exfiltrate the just-attached key. Returning the 3xx turns any redirect into a
// non-200 error. A caller-supplied client via WithHTTPClient owns its own policy.
func NewBase(defaultName string, defaultCaps ports.Capabilities, opts ...Option) Base {
	b := Base{
		Name: defaultName,
		Caps: defaultCaps,
		HTTPClient: &http.Client{
			Timeout:       DefaultHTTPTimeout,
			CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
		},
	}
	for _, o := range opts {
		o(&b)
	}
	return b
}

// Authorize applies the configured authorizer to req, if any.
func (b Base) Authorize(ctx context.Context, req *http.Request) error {
	if b.Auth == nil {
		return nil
	}
	return b.Auth.Apply(ctx, b.SecretRef, req)
}

// Send delivers a chunk on out, honoring ctx; it returns false if ctx is done
// (so the caller stops draining the upstream stream).
func Send(ctx context.Context, out chan<- core.LLMChunk, c core.LLMChunk) bool {
	select {
	case <-ctx.Done():
		return false
	case out <- c:
		return true
	}
}

// ReadSnippet reads up to 512 bytes of an error response body for a diagnostic
// message.
func ReadSnippet(r io.Reader) string {
	buf := make([]byte, 512)
	n, _ := r.Read(buf)
	return strings.TrimSpace(string(buf[:n]))
}

// ScanSSE reads newline-delimited SSE from r, invoking onData with each payload
// after stripping the "data:" prefix and skipping blank / non-data lines and the
// terminal "[DONE]" sentinel. It stops early (returning nil) if onData returns
// false — the adapter's signal that a downstream send was cancelled. It returns
// a non-nil error ONLY when the stream ended uncleanly while ctx is still live (a
// truncated read or an over-long line), so the caller surfaces a real provider
// failure as an error chunk; a clean end or a ctx cancellation returns nil.
func ScanSSE(ctx context.Context, r io.Reader, onData func(data []byte) bool) error {
	sc := bufio.NewScanner(r)
	// 4 MiB max line, matching the MCP/control scanners: a single large data:
	// line (e.g. a big Gemini functionCall) must not surface as a truncated
	// stream error.
	sc.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
	for sc.Scan() {
		if ctx.Err() != nil {
			return nil
		}
		line := strings.TrimSpace(sc.Text())
		if line == "" || !strings.HasPrefix(line, "data:") {
			continue
		}
		data := strings.TrimSpace(strings.TrimPrefix(line, "data:"))
		if data == "[DONE]" {
			break
		}
		if !onData([]byte(data)) {
			return nil
		}
	}
	if err := sc.Err(); err != nil && ctx.Err() == nil {
		return err
	}
	return nil
}

// ToolAccumulator reassembles streamed tool-call fragments into whole calls. It
// serves all adapters: OpenAI streams id/name/args fragments tagged by index;
// Anthropic sets id/name once (content_block_start) then appends args
// (input_json_delta) by block index; Google sends whole calls (one Add each).
// Calls() returns them sorted by index with empty args normalized to "{}".
type ToolAccumulator struct {
	byIndex map[int]*accCall
	order   []int
}

type accCall struct {
	id   string
	name string
	args strings.Builder
}

// NewToolAccumulator returns an empty accumulator.
func NewToolAccumulator() *ToolAccumulator {
	return &ToolAccumulator{byIndex: make(map[int]*accCall)}
}

func (a *ToolAccumulator) at(index int) *accCall {
	c, ok := a.byIndex[index]
	if !ok {
		c = &accCall{}
		a.byIndex[index] = c
		a.order = append(a.order, index)
	}
	return c
}

// Add merges a fragment for the call at index: a non-empty id or name overwrites
// (last wins, matching the providers), and an args fragment is appended.
func (a *ToolAccumulator) Add(index int, id, name, argsFragment string) {
	c := a.at(index)
	if id != "" {
		c.id = id
	}
	if name != "" {
		c.name = name
	}
	if argsFragment != "" {
		c.args.WriteString(argsFragment)
	}
}

// Len reports how many distinct calls have been seen.
func (a *ToolAccumulator) Len() int { return len(a.order) }

// Calls returns the accumulated calls sorted by index, with empty args
// normalized to the empty JSON object.
func (a *ToolAccumulator) Calls() []core.ToolCall {
	idxs := append([]int(nil), a.order...)
	sort.Ints(idxs)
	var out []core.ToolCall
	for _, idx := range idxs {
		c := a.byIndex[idx]
		args := c.args.String()
		if args == "" {
			args = "{}"
		}
		out = append(out, core.ToolCall{ID: c.id, Name: c.name, Args: json.RawMessage(args)})
	}
	return out
}
