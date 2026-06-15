package browser

import (
	"sync"

	"github.com/chromedp/cdproto/network"
	"github.com/chromedp/cdproto/page"
	"github.com/chromedp/chromedp"
)

// NetCall is one network request the page made, as reported by the `network`
// action. The capture is scoped to API traffic (XHR/fetch) — the calls that
// reveal a site's own endpoints — and carries metadata only, never bodies.
type NetCall struct {
	Method string `json:"method"`
	URL    string `json:"url"`
	Status int64  `json:"status,omitempty"`
	MIME   string `json:"mime,omitempty"`
}

const (
	// netCap bounds a tab's captured calls so a long-lived chatty page cannot
	// grow the log without limit; oldest calls fall off first.
	netCap = 300
	// netPendingCap bounds the in-flight request->method map, dropping it
	// wholesale if responses stop arriving (so abandoned requests can't leak).
	netPendingCap = 1024
)

// netLog is a tab's bounded record of API calls. The request event names the
// method; the response event carries the URL, status, and MIME — so the slice
// is built entirely at response time (no index into a slice survives an
// eviction). Its own mutex keeps the CDP event loop off the Browser's big lock,
// exactly like lastURL/urlMu.
type netLog struct {
	mu     sync.Mutex
	method map[network.RequestID]string
	calls  []NetCall
}

func newNetLog() *netLog { return &netLog{method: map[network.RequestID]string{}} }

func (l *netLog) onRequest(id network.RequestID, method string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if len(l.method) > netPendingCap {
		l.method = map[network.RequestID]string{}
	}
	l.method[id] = method
}

func (l *netLog) onResponse(id network.RequestID, url string, status int64, mime string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	method := l.method[id]
	delete(l.method, id)
	l.calls = append(l.calls, NetCall{Method: method, URL: url, Status: status, MIME: mime})
	if len(l.calls) > netCap {
		l.calls = append([]NetCall(nil), l.calls[len(l.calls)-netCap:]...)
	}
}

func (l *netLog) reset() {
	l.mu.Lock()
	l.method = map[network.RequestID]string{}
	l.calls = nil
	l.mu.Unlock()
}

func (l *netLog) snapshot() []NetCall {
	l.mu.Lock()
	defer l.mu.Unlock()
	return append([]NetCall(nil), l.calls...)
}

// isAPIType keeps the capture to the request kinds that carry a site's data —
// XHR and fetch — so the log surfaces endpoints, not the page's images/scripts.
func isAPIType(t network.ResourceType) bool {
	return t == network.ResourceTypeXHR || t == network.ResourceTypeFetch
}

// startNetworkCapture records the tab's API traffic into its bounded netLog so
// the agent can discover a site's own endpoints by observing the calls it makes
// rather than guessing URLs. The log resets on each top-level navigation, so it
// always reflects the current page. Best-effort: if enabling the Network domain
// fails, browsing is unaffected — there is simply nothing to read.
func (b *Browser) startNetworkCapture(t *tab) {
	if err := chromedp.Run(t.ctx, network.Enable()); err != nil {
		return
	}
	chromedp.ListenTarget(t.ctx, func(ev interface{}) {
		switch e := ev.(type) {
		case *network.EventRequestWillBeSent:
			if e.Request != nil && isAPIType(e.Type) {
				t.net.onRequest(e.RequestID, e.Request.Method)
			}
		case *network.EventResponseReceived:
			if e.Response != nil && isAPIType(e.Type) {
				t.net.onResponse(e.RequestID, e.Response.URL, e.Response.Status, e.Response.MimeType)
			}
		case *page.EventFrameNavigated:
			// The previous page's calls are stale on a new top-level document.
			// This listener owns the reset rather than the screencast one
			// because capture runs even when the screencast is off (no
			// StreamID), so it cannot rely on startScreencast's nav handling.
			if e.Frame != nil && e.Frame.ParentID == "" {
				t.net.reset()
			}
		}
	})
}

// Network returns the API (XHR/fetch) calls the active tab has captured since
// its last navigation, oldest first. The bool is false when no tab is open (the
// browser is not running) — distinct from a page that has made no API calls.
func (b *Browser) Network() ([]NetCall, bool) {
	b.mu.Lock()
	defer b.mu.Unlock()
	t, ok := b.tabs[b.active]
	if !ok {
		return nil, false
	}
	return t.net.snapshot(), true
}
