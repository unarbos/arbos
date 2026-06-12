package codingspec

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/tool"
)

const (
	defaultFetchTimeout = 30 * time.Second
	defaultFetchMaxBody = 256 * 1024
)

var (
	htmlTagRe = regexp.MustCompile(`(?s)<[^>]*>`)
	// errNoVault is what auth requests hit when no host wired a vault (CLI
	// one-shots, tests). Named so the message stays in one place.
	errNoVault = fmt.Errorf("no managed-secret vault is available in this arbos")
)

// FetchArgs are the arguments to fetch.
type FetchArgs struct {
	URL     string        `json:"url" desc:"HTTP or HTTPS URL to fetch."`
	Method  string        `json:"method,omitempty" desc:"HTTP method (GET or POST). Default GET."`
	Body    string        `json:"body,omitempty" desc:"Request body for POST."`
	Headers []fetchHeader `json:"headers,omitempty" desc:"Optional request headers."`
	Auth    string        `json:"auth,omitempty" desc:"Name of a managed secret to attach as credentials (Authorization: Bearer). The destination host must be on that secret's allowlist; HTTPS only. You never see the value."`
}

type fetchHeader struct {
	Name  string `json:"name" desc:"Header name."`
	Value string `json:"value" desc:"Header value."`
}

func fetchSpec() tool.Spec {
	return tool.NewSpec("fetch",
		"Fetch a URL and return the response body as text. Responses are truncated to 256KB. HTML tags are stripped for text/html responses. To call an API that needs credentials, pass the managed secret's name as `auth` — the value is attached at the boundary (host-allowlisted, HTTPS only) and never enters your context.",
		true,
		func(ctx context.Context, a FetchArgs) (string, error) {
			if strings.TrimSpace(a.URL) == "" {
				return "", fmt.Errorf("fetch: url is required")
			}
			method := strings.ToUpper(strings.TrimSpace(a.Method))
			if method == "" {
				method = http.MethodGet
			}
			if method != http.MethodGet && method != http.MethodPost {
				return "", fmt.Errorf("fetch: unsupported method %q (use GET or POST)", method)
			}

			var body io.Reader
			if a.Body != "" {
				body = strings.NewReader(a.Body)
			}
			req, err := http.NewRequestWithContext(ctx, method, a.URL, body)
			if err != nil {
				return "", fmt.Errorf("fetch: %w", err)
			}
			for _, h := range a.Headers {
				req.Header.Set(h.Name, h.Value)
			}
			if method == http.MethodPost && req.Header.Get("Content-Type") == "" {
				req.Header.Set("Content-Type", "application/json")
			}
			if name := strings.TrimSpace(a.Auth); name != "" {
				// Broker-attached credentials ride only TLS: a plaintext URL
				// would hand the value to every hop on the path.
				if req.URL.Scheme != "https" {
					return "", fmt.Errorf("fetch: auth %q requires an https URL", name)
				}
				if err := applySecret(ctx, name, req); err != nil {
					return "", fmt.Errorf("fetch: %w", err)
				}
			}

			cctx, cancel := context.WithTimeout(ctx, defaultFetchTimeout)
			defer cancel()
			resp, err := http.DefaultClient.Do(req.WithContext(cctx))
			if err != nil {
				return "", fmt.Errorf("fetch: %w", err)
			}
			defer func() { _ = resp.Body.Close() }()

			limited := io.LimitReader(resp.Body, defaultFetchMaxBody+1)
			data, err := io.ReadAll(limited)
			if err != nil {
				return "", fmt.Errorf("fetch: read body: %w", err)
			}
			truncated := len(data) > defaultFetchMaxBody
			if truncated {
				data = data[:defaultFetchMaxBody]
			}

			text := string(data)
			ct := resp.Header.Get("Content-Type")
			if strings.Contains(strings.ToLower(ct), "text/html") {
				text = strings.TrimSpace(htmlTagRe.ReplaceAllString(text, " "))
				text = regexp.MustCompile(`\s+`).ReplaceAllString(text, " ")
			}

			var b strings.Builder
			fmt.Fprintf(&b, "HTTP %d %s\nContent-Type: %s\n\n", resp.StatusCode, resp.Status, ct)
			b.WriteString(text)
			if truncated {
				b.WriteString("\n\n[truncated at 256KB]")
			}
			return b.String(), nil
		})
}
