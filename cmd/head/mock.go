package main

import (
	"context"
	"io"
	"net/http"
	"strings"
)

// mockUpstream is the inference forwarder for `--mock`: it returns a canned
// OpenAI-shaped reply (with a small fake cost so metering visibly debits)
// without any network or key. It implements head.Upstream.
type mockUpstream struct{}

func (mockUpstream) Name() string { return "mock" }

func (mockUpstream) Models(context.Context) (*http.Response, error) {
	body := `{"data":[{"id":"mock/echo","object":"model"}]}`
	return jsonResp(body, "application/json"), nil
}

func (mockUpstream) Forward(_ context.Context, _ []byte, stream bool) (*http.Response, error) {
	const reply = "Hello from the mock arbos head — your request was received and metered."
	if stream {
		body := `data: {"id":"mock-1","object":"chat.completion.chunk","model":"mock/echo","choices":[{"index":0,"delta":{"role":"assistant","content":"` + reply + `"}}]}` + "\n\n" +
			`data: {"id":"mock-1","object":"chat.completion.chunk","model":"mock/echo","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":14,"cost":0.0001}}` + "\n\n" +
			"data: [DONE]\n\n"
		return jsonResp(body, "text/event-stream"), nil
	}
	body := `{"id":"mock-1","object":"chat.completion","model":"mock/echo","choices":[{"index":0,"message":{"role":"assistant","content":"` + reply + `"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":14,"total_tokens":26,"cost":0.0001}}`
	return jsonResp(body, "application/json"), nil
}

func jsonResp(body, contentType string) *http.Response {
	return &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": {contentType}},
		Body:       io.NopCloser(strings.NewReader(body)),
	}
}
