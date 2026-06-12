package piwire

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Account-credits wiring: a thin closure over OpenRouter's /credits endpoint,
// built from the same resolved Config and secret-broker discipline as the
// chat provider and the audio closures. The gateway holds only the closure —
// never the key — and serves the totals to the Settings tab's provider panel.

// CreditsInfo is the account's lifetime totals in USD, as OpenRouter's
// /credits endpoint reports them; remaining = TotalCredits - TotalUsage.
type CreditsInfo struct {
	TotalCredits float64 `json:"total_credits"`
	TotalUsage   float64 `json:"total_usage"`
}

// creditsClient bounds one totals fetch — a tiny JSON blob, so generous.
var creditsClient = &http.Client{
	Timeout:       15 * time.Second,
	CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
}

// CreditsFetcher returns the account-credits closure the gateway serves, or
// nil when the configured base has no /credits endpoint (it is OpenRouter's
// API surface, not part of the generic OpenAI-compatible contract).
func (c Config) CreditsFetcher() func(ctx context.Context) (CreditsInfo, error) {
	if !c.openRouterHosted() {
		return nil
	}
	a := c.audioClient() // reused for its broker + ref; requests go out below
	return func(ctx context.Context) (CreditsInfo, error) {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, a.base+"/credits", nil)
		if err != nil {
			return CreditsInfo{}, err
		}
		if err := a.broker.Apply(ctx, a.ref, req); err != nil {
			return CreditsInfo{}, err
		}
		resp, err := creditsClient.Do(req)
		if err != nil {
			return CreditsInfo{}, err
		}
		defer func() { _ = resp.Body.Close() }()
		if resp.StatusCode != http.StatusOK {
			snippet, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
			return CreditsInfo{}, fmt.Errorf("/credits: status %d: %s", resp.StatusCode, strings.TrimSpace(string(snippet)))
		}
		var out struct {
			Data CreditsInfo `json:"data"`
		}
		if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
			return CreditsInfo{}, fmt.Errorf("decode credits: %w", err)
		}
		return out.Data, nil
	}
}
