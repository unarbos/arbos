// Package modelcatalog fetches the provider's live model catalog — the
// OpenAI-compatible {base}/models listing OpenRouter and the OpenAI API both
// serve. It is the one definition of "which models exist here", shared by
// every surface that offers a model choice: the gateway's /api/models proxy
// (the web composer's picker) and the agent's own list_models/set_model
// tools, so the user and the agent see the same catalog.
package modelcatalog

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"time"
)

// Model is one selectable model from the provider's catalog: the id (the
// exact string the provider accepts and set_model takes), a friendly display
// name, and the context window. JSON tags match the gateway's /api/models
// response shape, which the web picker renders directly.
type Model struct {
	ID            string `json:"id"`
	Name          string `json:"name,omitempty"`
	ContextLength int    `json:"context_length,omitempty"`
}

// client bounds the catalog fetch so a slow provider can't wedge a caller;
// the listing is a small JSON blob, so the timeout is generous.
var client = &http.Client{Timeout: 10 * time.Second}

// Fetch reads an OpenAI-compatible /models listing (the shape OpenRouter and
// the OpenAI API both return: {"data":[{id,name,...}]}). The endpoint is
// public on OpenRouter; no auth header is sent.
func Fetch(ctx context.Context, url string) ([]Model, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("models catalog: %s", resp.Status)
	}
	var body struct {
		Data []Model `json:"data"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		return nil, err
	}
	return body.Data, nil
}
