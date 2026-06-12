package browser

import (
	"crypto/sha256"
	"encoding/hex"
	"sync"
)

// StreamIDFor returns the screencast stream id for a workspace root — the
// single home for the derivation, shared by the tool layer (which publishes
// under it) and the gateway (which lets the UI subscribe to it), so the two
// can never drift.
func StreamIDFor(root string) string {
	sum := sha256.Sum256([]byte(root))
	return hex.EncodeToString(sum[:8])
}

// Update is one screencast message: a rendered frame (base64 JPEG), a
// navigation (the page's current URL), or both. Frames are frequent and URLs
// change rarely; carrying them on one stream keeps the viewer protocol to a
// single message shape.
type Update struct {
	Frame string `json:"frame,omitempty"`
	URL   string `json:"url,omitempty"`
}

// InputEvent is one user interaction forwarded from the screencast panel back
// into the live browser: a click or scroll at a position normalized to the
// frame (0..1, so the viewer never needs to know the real viewport size), a
// special key, or a run of typed text. This is what makes the panel a browser
// you can use — most importantly, to sign into a site yourself so the agent's
// persistent profile holds the session.
type InputEvent struct {
	T   string  `json:"t"`             // click | wheel | key | text | nav
	X   float64 `json:"x,omitempty"`   // normalized [0,1]
	Y   float64 `json:"y,omitempty"`   // normalized [0,1]
	DY  float64 `json:"dy,omitempty"`  // wheel delta, CSS px
	Key string  `json:"key,omitempty"` // DOM key name (Enter, Backspace, …)
	S   string  `json:"s,omitempty"`   // printable text, or nav's address/query
}

// frameHub fans out screencast updates from a Browser to any number of
// viewers, keyed by stream id, and routes viewer input back. It is the one
// shared seam between the tool layer (which owns the Browser, deep under the
// engine) and the gateway (which owns the viewer WebSocket) — neither imports
// the other, both import this. A process-level singleton because a Browser is
// created far from any gateway and cannot be handed a sink.
//
// It caches the last frame and URL per stream so a viewer connecting
// mid-session paints (and shows where it is) immediately instead of waiting
// for the next visual change (CDP emits a frame only when the page changes).
type frameHub struct {
	mu      sync.Mutex
	subs    map[string]map[chan Update]struct{}
	lastImg map[string]string
	lastURL map[string]string
	input   map[string]func(InputEvent)
}

// Frames is the process-wide screencast hub.
var Frames = &frameHub{
	subs:    map[string]map[chan Update]struct{}{},
	lastImg: map[string]string{},
	lastURL: map[string]string{},
	input:   map[string]func(InputEvent){},
}

// SetInput registers the live browser's input handler for a stream (replacing
// any previous one) and returns an unregister func. Input arriving with no
// handler (browser idle-closed, not yet launched) is dropped — the next agent
// action relaunches it.
func (h *frameHub) SetInput(id string, fn func(InputEvent)) func() {
	h.mu.Lock()
	h.input[id] = fn
	h.mu.Unlock()
	return func() {
		h.mu.Lock()
		delete(h.input, id)
		h.mu.Unlock()
	}
}

// Input forwards a viewer interaction to the stream's live browser, if any.
func (h *frameHub) Input(id string, ev InputEvent) {
	h.mu.Lock()
	fn := h.input[id]
	h.mu.Unlock()
	if fn != nil {
		fn(ev)
	}
}

// Forget drops a stream's cached frame and URL. Called when its tab closes —
// tab ids are never reused, so without this every closed tab would leave its
// last JPEG cached in the hub for the life of the process.
func (h *frameHub) Forget(id string) {
	h.mu.Lock()
	delete(h.lastImg, id)
	delete(h.lastURL, id)
	h.mu.Unlock()
}

// Publish delivers an update to every viewer of id and caches its parts for
// late joiners. A viewer whose buffer is full skips this update (the browser
// is never back-pressured by a slow watcher — dropping frames is the right
// failure for a live view).
func (h *frameHub) Publish(id string, u Update) {
	h.mu.Lock()
	if u.Frame != "" {
		h.lastImg[id] = u.Frame
	}
	if u.URL != "" {
		h.lastURL[id] = u.URL
	}
	chans := make([]chan Update, 0, len(h.subs[id]))
	for c := range h.subs[id] {
		chans = append(chans, c)
	}
	h.mu.Unlock()
	for _, c := range chans {
		select {
		case c <- u:
		default:
		}
	}
}

// Subscribe returns a channel of updates for id and an unsubscribe func. The
// cached last frame and URL are delivered immediately so the viewer is never
// blank or address-less.
func (h *frameHub) Subscribe(id string) (<-chan Update, func()) {
	ch := make(chan Update, 2)
	h.mu.Lock()
	if h.subs[id] == nil {
		h.subs[id] = map[chan Update]struct{}{}
	}
	h.subs[id][ch] = struct{}{}
	snapshot := Update{Frame: h.lastImg[id], URL: h.lastURL[id]}
	h.mu.Unlock()
	if snapshot.Frame != "" || snapshot.URL != "" {
		ch <- snapshot // buffered (cap 2); the first send never blocks
	}
	return ch, func() {
		h.mu.Lock()
		if m := h.subs[id]; m != nil {
			delete(m, ch)
			if len(m) == 0 {
				delete(h.subs, id)
			}
		}
		h.mu.Unlock()
	}
}
