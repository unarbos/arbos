package codingspec

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"html"
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
	// scriptStyleRe drops the content of tags that hold code, not prose — the
	// usual source of the "stripped HTML" mush (inline JS, CSS, JSON state).
	scriptStyleRe = regexp.MustCompile(`(?is)<(script|style|noscript|template)\b[^>]*>.*?</\s*(script|style|noscript|template)\s*>`)
	// blockBreakRe turns block-level boundaries into newlines so the extracted
	// text keeps the page's paragraph/list/row structure instead of collapsing
	// to one line.
	blockBreakRe = regexp.MustCompile(`(?i)</?(p|div|section|article|header|footer|nav|li|tr|h[1-6]|br|hr|table|ul|ol|dl|blockquote|pre)\b[^>]*>`)
	htmlTagRe    = regexp.MustCompile(`(?s)<[^>]*>`)
	inlineWSRe   = regexp.MustCompile(`[ \t\f\v]+`)
	blankLinesRe = regexp.MustCompile(`\n{3,}`)
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
	Auth    string        `json:"auth,omitempty" desc:"Name of a managed secret to attach as credentials. The secret's own scheme is applied (Authorization: Bearer by default, or a custom header). The destination host must be on that secret's allowlist; HTTPS only. You never see the value."`
	Raw     bool          `json:"raw,omitempty" desc:"Return the response body verbatim, skipping HTML-to-text extraction and JSON pretty-printing. Use when you need the raw markup or an embedded data blob (e.g. a page's hydration state)."`
}

type fetchHeader struct {
	Name  string `json:"name" desc:"Header name."`
	Value string `json:"value" desc:"Header value."`
}

func fetchSpec() tool.Spec {
	return tool.NewSpec("fetch",
		"Fetch a URL and return the response body. Responses are truncated to 256KB. HTML is reduced to readable text (scripts and styles dropped, structure kept); JSON is pretty-printed. Pass `raw` to skip both. To call an API that needs credentials, pass the managed secret's name as `auth` — its configured scheme is applied at the boundary (host-allowlisted, HTTPS only) and never enters your context.",
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

			ct := resp.Header.Get("Content-Type")
			text := renderBody(data, ct, a.Raw)

			var b strings.Builder
			fmt.Fprintf(&b, "HTTP %d %s\nContent-Type: %s\n\n", resp.StatusCode, resp.Status, ct)
			b.WriteString(text)
			if truncated {
				b.WriteString("\n\n[truncated at 256KB]")
			}
			return b.String(), nil
		})
}

// renderBody turns a response body into the text the agent reads. Raw returns
// it verbatim; otherwise JSON is indented and HTML is reduced to readable text.
// Anything else (and JSON that does not parse — e.g. a truncated body) passes
// through unchanged.
func renderBody(data []byte, contentType string, raw bool) string {
	if raw {
		return string(data)
	}
	lc := strings.ToLower(contentType)
	switch {
	case strings.Contains(lc, "json"):
		var buf bytes.Buffer
		if json.Indent(&buf, data, "", "  ") == nil {
			return buf.String()
		}
		return string(data)
	case strings.Contains(lc, "html"):
		return htmlToText(string(data))
	default:
		return string(data)
	}
}

// htmlToText extracts readable text from HTML: it drops code-bearing tags
// (script/style/...), turns block boundaries into newlines, removes the
// remaining tags, decodes entities, and squeezes whitespace — so prose and
// structure survive while the markup and inline code do not.
func htmlToText(s string) string {
	s = scriptStyleRe.ReplaceAllString(s, " ")
	s = blockBreakRe.ReplaceAllString(s, "\n")
	s = htmlTagRe.ReplaceAllString(s, "")
	s = html.UnescapeString(s)
	lines := strings.Split(s, "\n")
	for i, ln := range lines {
		lines[i] = strings.TrimSpace(inlineWSRe.ReplaceAllString(ln, " "))
	}
	return strings.TrimSpace(blankLinesRe.ReplaceAllString(strings.Join(lines, "\n"), "\n\n"))
}
