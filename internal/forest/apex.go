package forest

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"
)

// The apex doubles as the install door: `curl https://arbos.life | bash`
// pipes the install script, browsers get a landing page. Scripts are proxied
// live from GitHub main so the head never needs a redeploy for script
// changes.
const (
	installScriptURL = "https://raw.githubusercontent.com/unarbos/arbos/main/scripts/install.sh"
	launcherURL      = "https://raw.githubusercontent.com/unarbos/arbos/main/scripts/arbos"

	scriptCacheTTL = 5 * time.Minute
)

var scriptClient = &http.Client{Timeout: 15 * time.Second}

type cachedScript struct {
	mu      sync.Mutex
	url     string
	body    []byte
	fetched time.Time
}

var (
	installScript = &cachedScript{url: installScriptURL}
	launcherScrpt = &cachedScript{url: launcherURL}
)

func (c *cachedScript) get(ctx context.Context) ([]byte, error) {
	c.mu.Lock()
	if len(c.body) > 0 && time.Since(c.fetched) < scriptCacheTTL {
		b := c.body
		c.mu.Unlock()
		return b, nil
	}
	c.mu.Unlock()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := scriptClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("fetch %s: %s", c.url, resp.Status)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, err
	}

	c.mu.Lock()
	c.body = body
	c.fetched = time.Now()
	c.mu.Unlock()
	return body, nil
}

func wantsInstallScript(r *http.Request) bool {
	accept := r.Header.Get("Accept")
	ua := r.Header.Get("User-Agent")
	return strings.Contains(ua, "curl") ||
		strings.Contains(ua, "wget") ||
		!strings.Contains(accept, "text/html")
}

func (h *Head) serveScript(w http.ResponseWriter, r *http.Request, s *cachedScript) {
	body, err := s.get(r.Context())
	if err != nil {
		h.cfg.Logf("forest: script fetch: %v", err)
		http.Error(w, "script unavailable", http.StatusBadGateway)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Cache-Control", "public, max-age=300")
	_, _ = w.Write(body)
}

func (h *Head) serveInstallScript(w http.ResponseWriter, r *http.Request) {
	h.serveScript(w, r, installScript)
}

func (h *Head) serveLauncher(w http.ResponseWriter, r *http.Request) {
	h.serveScript(w, r, launcherScrpt)
}

func (h *Head) serveStatic(w http.ResponseWriter, r *http.Request, name, contentType string) {
	b, err := siteFS.ReadFile("site/" + name)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", contentType)
	w.Header().Set("Cache-Control", "public, max-age=3600")
	_, _ = w.Write(b)
}

func (h *Head) serveLanding(w http.ResponseWriter, r *http.Request) {
	b, err := siteFS.ReadFile("site/index.html")
	if err != nil {
		h.mu.Lock()
		n := len(h.leases)
		h.mu.Unlock()
		_, _ = fmt.Fprintf(w, "arbos forest head — %d active lease(s)\n", n)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "public, max-age=300")
	_, _ = w.Write(b)
}
