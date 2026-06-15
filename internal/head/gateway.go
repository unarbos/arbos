package head

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"io"
	"math"
	"net/http"
	"sync"
	"time"
)

// maxRequestBody bounds an inbound chat-completions body. Generous for large
// multimodal prompts, but not unbounded.
const maxRequestBody = 32 << 20 // 32 MiB

// handleChatCompletions is the inference gateway: authenticate the arbos key,
// gate on a positive balance, forward verbatim to the upstream, stream the
// reply straight back, and settle the metered cost against the ledger when the
// stream ends. The agent talks to the head exactly as it would to OpenRouter —
// same OpenAI-compatible body — and never sees the upstream key.
func (h *Head) handleChatCompletions(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}

	bal, err := h.store.Balance(r.Context(), princ.AccountID)
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "balance lookup failed")
		return
	}
	// Prepaid: a non-positive balance can't start a call. The message is the
	// contract's quota_exhausted notice (account-api-contract.md), which the
	// node surfaces as a notice rather than a stack trace.
	if bal <= 0 {
		writeAPIErr(w, http.StatusPaymentRequired, "quota_exhausted",
			"balance exhausted — fund your account to continue: https://arbos.life/account")
		return
	}

	raw, err := io.ReadAll(io.LimitReader(r.Body, maxRequestBody))
	if err != nil {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "could not read body")
		return
	}
	body, stream, err := prepareBody(raw)
	if err != nil {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "body must be a JSON object")
		return
	}

	resp, err := h.upstream.Forward(r.Context(), body, stream)
	if err != nil {
		writeAPIErr(w, http.StatusBadGateway, "upstream_unreachable", "inference upstream unreachable")
		return
	}
	defer func() { _ = resp.Body.Close() }()

	// Pass the upstream status through; on an error status just relay the body
	// (no metering — nothing was generated).
	copyHeader(w.Header(), resp.Header)
	w.WriteHeader(resp.StatusCode)
	if resp.StatusCode != http.StatusOK {
		_, _ = io.Copy(w, resp.Body)
		return
	}

	var u usageResult
	if stream {
		u = h.relayStream(w, resp.Body)
	} else {
		u = h.relayJSON(w, resp.Body)
	}
	h.settle(princ, u)
}

// settle debits the metered cost and records the call. The ledger ref ties the
// debit to this upstream generation so a retry can't double-charge; a zero cost
// (upstream withheld accounting) records usage without a debit.
func (h *Head) settle(princ *Principal, u usageResult) {
	if u.model == "" && u.costMicro == 0 && u.promptTokens == 0 {
		return // nothing came back to meter
	}
	costMicro := h.priceWithMargin(u.costMicro)
	ref := "infer:" + u.id
	if u.id == "" {
		ref = "infer:" + newID("gen") // upstream gave no id; mint one so the ref stays unique
	}
	ctx := context.Background()
	if costMicro > 0 {
		_ = h.store.Debit(ctx, princ.AccountID, costMicro, reasonInference, ref, map[string]any{
			"model": u.model, "upstream": h.upstream.Name(),
			"prompt_tokens": u.promptTokens, "completion_tokens": u.completionTokens,
		})
	}
	h.store.RecordUsage(ctx, princ.AccountID, princ.KeyID, u.model, h.upstream.Name(),
		u.promptTokens, u.completionTokens, costMicro, ref)
}

// priceWithMargin applies the head's configured markup to the upstream cost.
// marginBps is basis points (100 = 1%); zero is a pure pass-through.
func (h *Head) priceWithMargin(upstreamMicro int64) int64 {
	if h.marginBps == 0 {
		return upstreamMicro
	}
	return upstreamMicro + upstreamMicro*int64(h.marginBps)/10_000
}

// usageResult is what one call costs us, extracted from the upstream reply.
type usageResult struct {
	id               string
	model            string
	promptTokens     int
	completionTokens int
	costMicro        int64 // upstream cost before margin
}

// prepareBody parses the inbound JSON, detects streaming, and turns on usage
// accounting so the upstream reports cost and token counts we can meter
// against. It rewrites the body rather than trusting the client to opt in.
func prepareBody(raw []byte) (body []byte, stream bool, err error) {
	var m map[string]any
	if err := json.Unmarshal(raw, &m); err != nil {
		return nil, false, err
	}
	stream, _ = m["stream"].(bool)
	// Ask the upstream to include cost + tokens in the response.
	m["usage"] = map[string]any{"include": true}
	if stream {
		// OpenAI-style: emit a final usage chunk at the end of the stream.
		m["stream_options"] = map[string]any{"include_usage": true}
	}
	out, err := json.Marshal(m)
	return out, stream, err
}

// relayStream copies an SSE stream straight to the client, flushing each chunk,
// while watching for the usage frame to meter the call. It never buffers the
// whole stream — the agent sees tokens as they arrive.
func (h *Head) relayStream(w http.ResponseWriter, body io.Reader) usageResult {
	flusher, _ := w.(http.Flusher)
	var u usageResult
	reader := bufio.NewReader(body)
	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > 0 {
			_, _ = w.Write(line)
			if flusher != nil {
				flusher.Flush()
			}
			if payload, ok := sseData(line); ok {
				mergeUsage(&u, payload)
			}
		}
		if err != nil {
			return u
		}
	}
}

// relayJSON copies a single JSON reply to the client and meters it. OpenRouter
// can emit SSE-style keepalive lines before the JSON body even on a
// non-streaming request, so meter from the first '{' rather than the raw head.
func (h *Head) relayJSON(w http.ResponseWriter, body io.Reader) usageResult {
	raw, _ := io.ReadAll(body)
	_, _ = w.Write(raw)
	var u usageResult
	if i := bytes.IndexByte(raw, '{'); i >= 0 {
		mergeUsage(&u, raw[i:])
	}
	return u
}

// sseData returns the JSON payload of a "data:" SSE line, false for blank
// lines, comments, and the terminal [DONE] sentinel.
func sseData(line []byte) ([]byte, bool) {
	t := bytes.TrimSpace(line)
	rest, ok := bytes.CutPrefix(t, []byte("data:"))
	if !ok {
		return nil, false
	}
	d := bytes.TrimSpace(rest)
	if len(d) == 0 || bytes.Equal(d, []byte("[DONE]")) {
		return nil, false
	}
	return d, true
}

// mergeUsage folds a chunk's usage/model into u when present. Streaming sends
// usage once at the end; a non-stream reply carries it inline. Token counts and
// the upstream cost are last-wins, which matches both shapes.
func mergeUsage(u *usageResult, payload []byte) {
	var p struct {
		ID    string `json:"id"`
		Model string `json:"model"`
		Usage *struct {
			PromptTokens     int     `json:"prompt_tokens"`
			CompletionTokens int     `json:"completion_tokens"`
			Cost             float64 `json:"cost"`
		} `json:"usage"`
	}
	if json.Unmarshal(payload, &p) != nil {
		return
	}
	if p.ID != "" {
		u.id = p.ID
	}
	if p.Model != "" {
		u.model = p.Model
	}
	if p.Usage != nil {
		u.promptTokens = p.Usage.PromptTokens
		u.completionTokens = p.Usage.CompletionTokens
		u.costMicro = int64(math.Round(p.Usage.Cost * microPerUSD))
	}
}

// handleModels proxies the upstream model list, cached briefly.
func (h *Head) handleModels(w http.ResponseWriter, r *http.Request) {
	if _, ok := h.requirePrincipal(w, r); !ok {
		return
	}
	if b, ok := h.modelsCache.get(); ok {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write(b)
		return
	}
	resp, err := h.upstream.Models(r.Context())
	if err != nil {
		writeAPIErr(w, http.StatusBadGateway, "upstream_unreachable", "model list unavailable")
		return
	}
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if resp.StatusCode == http.StatusOK {
		h.modelsCache.put(b)
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(resp.StatusCode)
	_, _ = w.Write(b)
}

// modelsCacheTTL bounds how stale the proxied model list may be; the catalog
// changes rarely, so a few minutes spares the upstream without going stale.
const modelsCacheTTL = 5 * time.Minute

// modelsCache is a tiny TTL cache for the proxied model list.
type modelsCache struct {
	mu      sync.Mutex
	body    []byte
	fetched time.Time
}

func (c *modelsCache) get() ([]byte, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.body) > 0 && time.Since(c.fetched) < modelsCacheTTL {
		return c.body, true
	}
	return nil, false
}

func (c *modelsCache) put(b []byte) {
	c.mu.Lock()
	c.body = b
	c.fetched = time.Now()
	c.mu.Unlock()
}

// copyHeader copies upstream response headers, minus hop-by-hop ones that would
// confuse our own connection handling.
func copyHeader(dst, src http.Header) {
	for k, vs := range src {
		switch k {
		case "Connection", "Transfer-Encoding", "Content-Length", "Keep-Alive":
			continue
		}
		for _, v := range vs {
			dst.Add(k, v)
		}
	}
}
