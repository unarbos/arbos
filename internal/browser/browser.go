// Package browser drives a real Chromium over the Chrome DevTools Protocol
// (chromedp) so the agent can render and act on pages a plain HTTP fetch can't:
// a JS-rendered localhost dev site, a screenshot the model actually sees, a
// click or a form fill. It is the stateful escalation of the `fetch` tool — to
// `fetch` what `bash` is to `read`.
//
// One Browser owns one Chromium process holding any number of tabs. The agent
// acts on its ACTIVE tab (new_tab/switch_tab move it); each tab screencasts on
// its own stream and takes viewer input on it, so every tab is independently
// watchable and usable from the UI. The process launches lazily on first use
// and tears down after an idle period (there is no session-end hook in the
// tool layer, so the browser must not leak a process when a session goes quiet
// — it mirrors the jobs philosophy: cheap, self-managing, nothing to clean up
// by hand). Calls are serialized by a mutex; the engine already serializes
// this tool (it mutates page state, so it is never ReadOnly), but the lock
// also guards the lazy launch, tab table, and idle teardown against each other.
package browser

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/chromedp/cdproto/browser"
	"github.com/chromedp/cdproto/emulation"
	"github.com/chromedp/cdproto/input"
	"github.com/chromedp/cdproto/page"
	"github.com/chromedp/chromedp"
	"github.com/chromedp/chromedp/kb"
)

// idleTimeout closes the Chromium instance after this long without a call, so an
// idle session does not hold a browser process open. The next call relaunches.
const idleTimeout = 5 * time.Minute

// defaultActionTimeout bounds a single browser action. A page that never fires
// its load event (or a selector that never appears) fails the call instead of
// wedging the session; the agent sees the error and adapts.
const defaultActionTimeout = 30 * time.Second

// viewportWidth/Height are the browser's default viewport — a full desktop
// layout, captured 1:1 by the screencast. The panel scales it down to fit, so
// the live view reads zoomed-out and dense (a usable browser), not a mobile-ish
// strip letterboxed in the dark.
const (
	viewportWidth  = 1600
	viewportHeight = 1000
)

// Config controls how a Browser launches and what it mirrors.
type Config struct {
	// Headless launches Chromium without a window (the default). Headful
	// (false, via ARBOS_BROWSER_HEADFUL) opens a real window — the escape
	// hatch for sites whose bot-detection defeats the interactive panel.
	Headless bool
	// ProfileDir is a persistent user-data directory. Set it so cookies and
	// logins survive across runs (the "stay signed into my accounts" mode);
	// empty uses a throwaway profile each launch. It is separate from the
	// user's everyday Chrome profile, so it never collides with it.
	ProfileDir string
	// StreamID is the workspace's screencast namespace: each tab publishes
	// frames to Frames under "<StreamID>/<tabID>", so every tab has its own
	// independently watchable (and drivable) stream. Empty disables the
	// screencast entirely.
	StreamID string
}

// ConfigFor derives the workspace's browser config from the environment —
// the single home for the policy, shared by the tool layer and the gateway.
// Default: headless with a persistent profile and per-tab screencasts (the
// "Browser" panels ARE the browser windows). ARBOS_BROWSER_HEADFUL opens a
// real window too, for the rare site whose bot-detection needs it.
func ConfigFor(root string) Config {
	profile := filepath.Join(os.TempDir(), "arbos-browser", "profile")
	if dir, err := os.UserConfigDir(); err == nil {
		profile = filepath.Join(dir, "arbos", "browser-profile")
	}
	return Config{
		Headless:   os.Getenv("ARBOS_BROWSER_HEADFUL") == "",
		ProfileDir: profile,
		StreamID:   StreamIDFor(root),
	}
}

// tab is one live Chrome tab: its chromedp context, its screencast stream, and
// its input unsubscriber.
type tab struct {
	id     string
	stream string
	ctx    context.Context
	cancel context.CancelFunc
	unsub  func()
}

// TabInfo is one tab's identity as reported to the tool and the UI.
type TabInfo struct {
	ID     string `json:"id"`
	Stream string `json:"stream"`
	URL    string `json:"url,omitempty"`
	Active bool   `json:"active,omitempty"`
}

// Browser is one Chromium process and its tabs. The zero value is not usable;
// build one with New (or Shared).
type Browser struct {
	cfg Config

	mu          sync.Mutex
	allocCancel context.CancelFunc
	// anchorCancel/anchorCtx hold chromedp's root context. Its target is never
	// used as a tab: cancelling the root kills the whole browser, so real tabs
	// are all children — uniformly closable without special-casing the first.
	anchorCancel context.CancelFunc
	anchorCtx    context.Context
	tabs         map[string]*tab
	order        []string
	active       string
	nextTab      int
	idleTimer    *time.Timer

	// userAgent is this browser's real Chrome User-Agent with any "Headless"
	// marker stripped, computed once at launch. Applied to every tab so the
	// page (and Sec-CH-UA client hints derived from it) reads as a normal
	// desktop Chrome rather than an automated/headless one — the first thing
	// bot-detection (Cloudflare, reCAPTCHA) keys on.
	userAgent string

	// urlMu guards lastURL separately from mu: navigation events update it from
	// chromedp's event loop, which must never block behind a long agent action
	// holding mu (that would stall the tab's screencast frames mid-action).
	urlMu   sync.Mutex
	lastURL map[string]string // tab id -> last seen top-level URL
}

// New returns a Browser configured by cfg, launched lazily on first use.
func New(cfg Config) *Browser {
	return &Browser{cfg: cfg, tabs: map[string]*tab{}, lastURL: map[string]string{}}
}

var (
	sharedMu sync.Mutex
	shared   = map[string]*Browser{}
)

// Shared returns the process-wide Browser for cfg's profile, creating it on
// first use. Every toolset that browses with the same profile (every session
// in a workspace, delegated children included) gets the SAME instance — a
// persistent profile admits one Chrome at a time (its ProcessSingleton lock),
// so per-instance browsers would fail the moment two sessions browse. One
// browser, shared tabs, every session and every panel looking at the same
// Chrome. A profile-less (throwaway) config gets a fresh instance, which
// cannot collide.
func Shared(cfg Config) *Browser {
	if cfg.ProfileDir == "" {
		return New(cfg)
	}
	sharedMu.Lock()
	defer sharedMu.Unlock()
	if b, ok := shared[cfg.ProfileDir]; ok {
		return b
	}
	b := New(cfg)
	shared[cfg.ProfileDir] = b
	return b
}

// ensure launches Chromium (with one tab) if it is not already running,
// reclaiming the profile from an orphaned previous browser if necessary.
// Caller holds b.mu.
func (b *Browser) ensure() error {
	if b.anchorCtx != nil {
		return nil
	}
	err := b.launch()
	if err != nil && b.cfg.ProfileDir != "" && strings.Contains(err.Error(), "ProcessSingleton") {
		// The profile is locked by a previous browser this process no longer
		// knows — typically an orphan from a killed arbos (the dev loop's
		// restart) whose Chrome outlived it. The profile dir is exclusively
		// arbos's, so the holder is ours by definition: reclaim and retry once.
		reclaimProfile(b.cfg.ProfileDir)
		err = b.launch()
	}
	return err
}

// launch starts Chromium and opens the first tab. Caller holds b.mu.
func (b *Browser) launch() error {
	opts := append(stealthAllocatorOptions(), chromedp.WindowSize(viewportWidth, viewportHeight))
	if b.cfg.Headless {
		// New headless ("--headless=new") shares the full rendering stack with
		// headful Chrome, so its fingerprint matches a real browser far more
		// closely than the legacy headless mode chromedp defaults to.
		opts = append(opts, chromedp.Flag("headless", "new"))
	}
	if b.cfg.ProfileDir != "" {
		opts = append(opts, chromedp.UserDataDir(b.cfg.ProfileDir))
	}
	allocCtx, allocCancel := chromedp.NewExecAllocator(context.Background(), opts...)
	anchorCtx, anchorCancel := chromedp.NewContext(allocCtx)
	// Run with no actions to actually launch the process now, so a launch
	// failure (no Chrome on the host) surfaces here rather than mid-action.
	if err := chromedp.Run(anchorCtx); err != nil {
		anchorCancel()
		allocCancel()
		if strings.Contains(err.Error(), "executable file not found") {
			// Make the missing-browser case actionable for the agent reading
			// this error: it has a shell and can install one itself.
			return fmt.Errorf("launch chromium: no Chrome/Chromium is installed on this host. "+
				"Install one and retry — Ubuntu/Debian: `sudo apt-get install -y chromium` or "+
				"`sudo snap install chromium` (Ubuntu's apt chromium-browser is a snap shim); "+
				"macOS: `brew install --cask google-chrome`. (%w)", err)
		}
		return fmt.Errorf("launch chromium: %w", err)
	}
	b.allocCancel = allocCancel
	b.anchorCancel = anchorCancel
	b.anchorCtx = anchorCtx
	// Capture the real UA once and strip the "Headless" marker, so every tab
	// can present a normal-Chrome User-Agent. Best-effort: a failure here just
	// leaves userAgent empty and tabs keep Chrome's default UA.
	_ = chromedp.Run(anchorCtx, chromedp.ActionFunc(func(ctx context.Context) error {
		if _, _, _, ua, _, err := browser.GetVersion().Do(ctx); err == nil {
			b.userAgent = strings.ReplaceAll(ua, "HeadlessChrome", "Chrome")
		}
		return nil
	}))
	if _, err := b.newTabLocked(); err != nil {
		b.closeLocked()
		return err
	}
	return nil
}

// newTabLocked opens a fresh tab in the running browser, wires its screencast
// and input stream, and makes it the active tab. Caller holds b.mu.
func (b *Browser) newTabLocked() (*tab, error) {
	b.nextTab++
	id := "t" + strconv.Itoa(b.nextTab)
	tctx, tcancel := chromedp.NewContext(b.anchorCtx)
	if err := chromedp.Run(tctx); err != nil {
		tcancel()
		return nil, fmt.Errorf("new tab: %w", err)
	}
	t := &tab{id: id, ctx: tctx, cancel: tcancel}
	b.applyStealth(tctx)
	if b.cfg.StreamID != "" {
		t.stream = b.cfg.StreamID + "/" + id
		b.startScreencast(t)
		t.unsub = Frames.SetInput(t.stream, func(ev InputEvent) { b.handleInput(t, ev) })
	}
	// Foreground the new tab so its screencast paints immediately (background
	// targets emit no frames) — a fresh panel should never open onto black.
	_ = chromedp.Run(tctx, page.BringToFront())
	b.tabs[id] = t
	b.order = append(b.order, id)
	b.active = id
	return t, nil
}

// reclaimProfile frees an arbos profile dir held by a browser this process
// does not own: kill the holder recorded in Chrome's SingletonLock (the
// symlink's target is "host-pid"), then clear the singleton files. Safe by
// construction — the dir is arbos's alone, so any holder is an orphaned arbos
// browser, never the user's own Chrome (which has its own profile).
func reclaimProfile(dir string) {
	lock := filepath.Join(dir, "SingletonLock")
	if target, err := os.Readlink(lock); err == nil {
		if i := strings.LastIndex(target, "-"); i >= 0 {
			pid, perr := strconv.Atoi(target[i+1:])
			host, _ := os.Hostname()
			// Kill only when the lock names THIS host: a lock minted elsewhere
			// (shared/NFS home, host rename) refers to some other machine's
			// pid, and killing the same number locally would hit an innocent
			// process. A foreign lock is stale here either way — removing the
			// files below is the whole reclaim.
			if perr == nil && pid > 0 && target[:i] == host {
				if syscall.Kill(pid, syscall.SIGKILL) == nil {
					// Wait for the holder to actually die before the relaunch
					// grabs the profile — a fixed beat loses the race against a
					// whole Chrome process family tearing down, and the retry
					// then fails on the still-held lock.
					for i := 0; i < 20 && syscall.Kill(pid, 0) == nil; i++ {
						time.Sleep(100 * time.Millisecond)
					}
				}
			}
		}
	}
	_ = os.Remove(lock)
	_ = os.Remove(filepath.Join(dir, "SingletonSocket"))
	_ = os.Remove(filepath.Join(dir, "SingletonCookie"))
}

/* -------------------------------- stealth ------------------------------- */

// stealthAllocatorOptions is chromedp's default launch flags with the two that
// brand the browser as automated removed: --enable-automation (which trips
// navigator.webdriver) is dropped entirely, and --disable-blink-features=
// AutomationControlled is added so the page sees navigator.webdriver === false.
// Everything else mirrors chromedp's (Puppeteer-derived) defaults so stability
// is unchanged. Headless mode is chosen by the caller, not here.
func stealthAllocatorOptions() []chromedp.ExecAllocatorOption {
	return []chromedp.ExecAllocatorOption{
		chromedp.NoFirstRun,
		chromedp.NoDefaultBrowserCheck,
		chromedp.Flag("disable-blink-features", "AutomationControlled"),
		chromedp.Flag("disable-background-networking", true),
		chromedp.Flag("enable-features", "NetworkService,NetworkServiceInProcess"),
		chromedp.Flag("disable-background-timer-throttling", true),
		chromedp.Flag("disable-backgrounding-occluded-windows", true),
		chromedp.Flag("disable-breakpad", true),
		chromedp.Flag("disable-client-side-phishing-detection", true),
		chromedp.Flag("disable-default-apps", true),
		chromedp.Flag("disable-dev-shm-usage", true),
		chromedp.Flag("disable-extensions", true),
		chromedp.Flag("disable-features", "site-per-process,Translate,BlinkGenPropertyTrees"),
		chromedp.Flag("disable-hang-monitor", true),
		chromedp.Flag("disable-ipc-flooding-protection", true),
		chromedp.Flag("disable-popup-blocking", true),
		chromedp.Flag("disable-prompt-on-repost", true),
		chromedp.Flag("disable-renderer-backgrounding", true),
		chromedp.Flag("disable-sync", true),
		chromedp.Flag("force-color-profile", "srgb"),
		chromedp.Flag("metrics-recording-only", true),
		chromedp.Flag("safebrowsing-disable-auto-update", true),
		chromedp.Flag("password-store", "basic"),
		chromedp.Flag("use-mock-keychain", true),
	}
}

// chromeVersionRE pulls the Chrome version out of a User-Agent string
// ("…Chrome/126.0.6478.127 Safari…") so the client-hint metadata we synthesize
// matches the UA we present — a mismatch between the two is itself a tell.
var chromeVersionRE = regexp.MustCompile(`Chrome/([\d.]+)`)

// applyStealth makes one fresh tab present as an ordinary desktop Chrome:
// it overrides the User-Agent (with matching Accept-Language, platform, and
// Sec-CH-UA client hints) and installs a script — run before any page script
// on every navigation — that patches the handful of JS surfaces bot-detection
// inspects (navigator.webdriver, window.chrome, plugins, languages,
// permissions). Best-effort: failures leave the tab usable, just less
// disguised. Caller holds b.mu (called from newTabLocked).
func (b *Browser) applyStealth(tctx context.Context) {
	_ = chromedp.Run(tctx, chromedp.ActionFunc(func(ctx context.Context) error {
		if b.userAgent != "" {
			ua := emulation.SetUserAgentOverride(b.userAgent).
				WithAcceptLanguage("en-US,en;q=0.9").
				WithPlatform(chPlatform()).
				WithUserAgentMetadata(uaMetadata(b.userAgent))
			_ = ua.Do(ctx)
		}
		_, err := page.AddScriptToEvaluateOnNewDocument(stealthJS).Do(ctx)
		return err
	}))
}

// uaMetadata synthesizes Sec-CH-UA client hints consistent with ua, so the
// high-entropy client hints a site can request match the navigator.userAgent
// string we present.
func uaMetadata(ua string) *emulation.UserAgentMetadata {
	full := "120.0.0.0"
	if m := chromeVersionRE.FindStringSubmatch(ua); len(m) == 2 {
		full = m[1]
	}
	major := full
	if i := strings.IndexByte(full, '.'); i >= 0 {
		major = full[:i]
	}
	brands := []*emulation.UserAgentBrandVersion{
		{Brand: "Chromium", Version: major},
		{Brand: "Google Chrome", Version: major},
		{Brand: "Not/A)Brand", Version: "24"},
	}
	fullList := []*emulation.UserAgentBrandVersion{
		{Brand: "Chromium", Version: full},
		{Brand: "Google Chrome", Version: full},
		{Brand: "Not/A)Brand", Version: "24.0.0.0"},
	}
	arch := "x86"
	if strings.HasPrefix(runtime.GOARCH, "arm") {
		arch = "arm"
	}
	return &emulation.UserAgentMetadata{
		Brands:          brands,
		FullVersionList: fullList,
		Platform:        chPlatform(),
		Architecture:    arch,
		Bitness:         "64",
	}
}

// chPlatform maps the host OS to the value Chrome reports in
// navigator.platform / Sec-CH-UA-Platform.
func chPlatform() string {
	switch runtime.GOOS {
	case "darwin":
		return "macOS"
	case "windows":
		return "Windows"
	default:
		return "Linux"
	}
}

// stealthJS patches the JS surfaces automated Chrome differs from a human's on,
// running before any page script on every document. With
// --disable-blink-features=AutomationControlled navigator.webdriver is already
// false; the rest (window.chrome, a non-empty plugins array, languages, the
// notifications permission quirk) closes the other common headless tells.
const stealthJS = `
(() => {
  try { Object.defineProperty(navigator, 'webdriver', { get: () => false }); } catch (e) {}
  try {
    if (!window.chrome) { window.chrome = {}; }
    if (!window.chrome.runtime) { window.chrome.runtime = {}; }
  } catch (e) {}
  try {
    Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en'] });
  } catch (e) {}
  try {
    if (!navigator.plugins || navigator.plugins.length === 0) {
      Object.defineProperty(navigator, 'plugins', { get: () => [1, 2, 3, 4, 5] });
    }
  } catch (e) {}
  try {
    const orig = navigator.permissions && navigator.permissions.query;
    if (orig) {
      navigator.permissions.query = (p) =>
        p && p.name === 'notifications'
          ? Promise.resolve({ state: Notification.permission })
          : orig(p);
    }
  } catch (e) {}
})();
`

// startScreencast wires one tab's live CDP screencast: every rendered frame is
// published to Frames under the tab's stream, and acked so Chrome keeps
// sending. Frames flow only when the page actually changes, so a static tab
// costs nothing. Navigation events ride the same stream as URL updates, so the
// viewer's address bar tracks the page. Best-effort — a screencast failure
// must not break browsing.
func (b *Browser) startScreencast(t *tab) {
	chromedp.ListenTarget(t.ctx, func(ev interface{}) {
		switch e := ev.(type) {
		case *page.EventScreencastFrame:
			Frames.Publish(t.stream, Update{Frame: e.Data})
			// Ack on its own goroutine: the listener runs on chromedp's event
			// loop, so a synchronous Run here would deadlock. Without the ack
			// Chrome stops after the first frame.
			go func(sid int64) {
				_ = chromedp.Run(t.ctx, page.ScreencastFrameAck(sid))
			}(e.SessionID)
		case *page.EventFrameNavigated:
			// Top-level document navigations only; iframes have a parent.
			if e.Frame.ParentID == "" {
				b.setURL(t.id, e.Frame.URL)
				Frames.Publish(t.stream, Update{URL: e.Frame.URL})
				go b.publishNavState(t)
			}
		case *page.EventNavigatedWithinDocument:
			// SPA route changes (history.pushState, hash) never fire
			// frameNavigated, but the address still moved.
			b.setURL(t.id, e.URL)
			Frames.Publish(t.stream, Update{URL: e.URL})
			go b.publishNavState(t)
		}
	})
	_ = chromedp.Run(t.ctx, page.StartScreencast().
		WithFormat("jpeg").
		WithQuality(70).
		WithMaxWidth(viewportWidth).
		WithMaxHeight(viewportHeight))
}

func (b *Browser) setURL(tabID, u string) {
	b.urlMu.Lock()
	b.lastURL[tabID] = u
	b.urlMu.Unlock()
}

// publishNavState reads the tab's navigation history and publishes whether it
// can go back/forward, so the viewer's controls grey out at the ends of
// history. Run on its own goroutine off the navigation listener: the CDP
// round-trip would deadlock chromedp's event loop if done inline (the same
// reason the screencast ack is goroutine'd). Best-effort — a failure just
// leaves the controls in their last state until the next navigation.
func (b *Browser) publishNavState(t *tab) {
	var cur int64
	var entries []*page.NavigationEntry
	err := chromedp.Run(t.ctx, chromedp.ActionFunc(func(ctx context.Context) error {
		var e error
		cur, entries, e = page.GetNavigationHistory().Do(ctx)
		return e
	}))
	if err != nil {
		return
	}
	Frames.Publish(t.stream, Update{Nav: &NavState{
		CanBack:    cur > 0,
		CanForward: cur < int64(len(entries)-1),
	}})
}

/* ---------------------------- tab management ---------------------------- */

// NewTab opens a fresh tab (launching the browser if needed) and makes it the
// agent's active tab. When the call itself launched the browser, the launch's
// first tab IS the fresh tab — creating another would hand every cold start a
// stray blank sibling.
func (b *Browser) NewTab(ctx context.Context) (TabInfo, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	hadTabs := len(b.tabs) > 0
	if err := b.ensure(); err != nil {
		return TabInfo{}, err
	}
	b.touch()
	if !hadTabs {
		t := b.tabs[b.active]
		return TabInfo{ID: t.id, Stream: t.stream, Active: true}, nil
	}
	t, err := b.newTabLocked()
	if err != nil {
		return TabInfo{}, err
	}
	return TabInfo{ID: t.id, Stream: t.stream, Active: true}, nil
}

// SwitchTab makes an existing tab the agent's active tab.
func (b *Browser) SwitchTab(ctx context.Context, id string) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	if _, ok := b.tabs[id]; !ok {
		return fmt.Errorf("no such tab %q (use tabs to list)", id)
	}
	b.touch()
	b.active = id
	return nil
}

// CloseTab closes one tab. Closing the last tab closes the whole browser.
// Closing the active tab activates the most recently opened survivor.
func (b *Browser) CloseTab(ctx context.Context, id string) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	t, ok := b.tabs[id]
	if !ok {
		return fmt.Errorf("no such tab %q (use tabs to list)", id)
	}
	b.closeTabLocked(t)
	if len(b.tabs) == 0 {
		b.closeLocked()
		return nil
	}
	if b.active == id {
		b.active = b.order[len(b.order)-1]
	}
	b.touch()
	return nil
}

func (b *Browser) closeTabLocked(t *tab) {
	if t.unsub != nil {
		t.unsub()
	}
	if t.stream != "" {
		Frames.Forget(t.stream)
	}
	t.cancel()
	delete(b.tabs, t.id)
	for i, id := range b.order {
		if id == t.id {
			b.order = append(b.order[:i], b.order[i+1:]...)
			break
		}
	}
	b.urlMu.Lock()
	delete(b.lastURL, t.id)
	b.urlMu.Unlock()
}

// Tabs lists the open tabs, oldest first. It never launches the browser: a
// browser that isn't running simply has no tabs.
func (b *Browser) Tabs() []TabInfo {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.urlMu.Lock()
	defer b.urlMu.Unlock()
	out := make([]TabInfo, 0, len(b.order))
	for _, id := range b.order {
		t := b.tabs[id]
		out = append(out, TabInfo{
			ID:     t.id,
			Stream: t.stream,
			URL:    b.lastURL[id],
			Active: id == b.active,
		})
	}
	return out
}

// ActiveStream names the active tab's screencast stream ("" when the browser
// is not running) — what the tool's live-panel reference points at.
func (b *Browser) ActiveStream() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	if t, ok := b.tabs[b.active]; ok {
		return t.stream
	}
	return ""
}

/* ------------------------------- lifecycle ------------------------------ */

// touch resets the idle-teardown timer. Caller holds b.mu.
func (b *Browser) touch() {
	if b.idleTimer != nil {
		b.idleTimer.Stop()
	}
	b.idleTimer = time.AfterFunc(idleTimeout, b.Close)
}

// Close tears down the Chromium instance and every tab. Safe to call
// repeatedly and from the idle timer; the next action relaunches.
func (b *Browser) Close() {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.closeLocked()
}

func (b *Browser) closeLocked() {
	if b.idleTimer != nil {
		b.idleTimer.Stop()
		b.idleTimer = nil
	}
	for _, t := range b.tabs {
		if t.unsub != nil {
			t.unsub()
		}
		if t.stream != "" {
			Frames.Forget(t.stream)
		}
		t.cancel()
	}
	b.tabs = map[string]*tab{}
	b.order = nil
	b.active = ""
	b.urlMu.Lock()
	b.lastURL = map[string]string{}
	b.urlMu.Unlock()
	if b.anchorCancel != nil {
		b.anchorCancel()
		b.anchorCancel = nil
	}
	if b.allocCancel != nil {
		b.allocCancel()
		b.allocCancel = nil
	}
	b.anchorCtx = nil
}

/* -------------------------------- actions ------------------------------- */

// run executes a batch of chromedp actions against the ACTIVE tab, launching
// the browser if needed, bounding the batch with a timeout, and cancelling it
// if the turn is interrupted (callCtx done). It is the single choke point every
// agent action goes through, so launch, idle-bookkeeping, timeout, and
// interrupt handling live in one place.
func (b *Browser) run(callCtx context.Context, actions ...chromedp.Action) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	if err := b.ensure(); err != nil {
		return err
	}
	t, ok := b.tabs[b.active]
	if !ok {
		return fmt.Errorf("no active tab")
	}
	return b.runOnLocked(callCtx, t, actions...)
}

// runOnLocked executes actions against one tab. Caller holds b.mu.
//
// Every batch leads with BringToFront: tabs live behind a hidden anchor
// target, and Chrome neither paints nor screencasts a background tab — without
// foregrounding, the live panel of the tab being acted on would never receive
// a frame. Frames follow interaction, exactly like a real browser's foreground
// tab; idle tabs' panels keep their last frame from the hub cache.
func (b *Browser) runOnLocked(callCtx context.Context, t *tab, actions ...chromedp.Action) error {
	b.touch()
	tctx, cancel := context.WithTimeout(t.ctx, defaultActionTimeout)
	defer cancel()
	// An interrupt (callCtx) cancels the in-flight action, not just the turn.
	stop := context.AfterFunc(callCtx, cancel)
	defer stop()
	return chromedp.Run(tctx, append([]chromedp.Action{page.BringToFront()}, actions...)...)
}

// specialKeys maps the DOM key names the panel sends to chromedp's key codes.
var specialKeys = map[string]string{
	"Enter":      kb.Enter,
	"Backspace":  kb.Backspace,
	"Tab":        kb.Tab,
	"Escape":     kb.Escape,
	"Delete":     kb.Delete,
	"ArrowUp":    kb.ArrowUp,
	"ArrowDown":  kb.ArrowDown,
	"ArrowLeft":  kb.ArrowLeft,
	"ArrowRight": kb.ArrowRight,
	"Home":       kb.Home,
	"End":        kb.End,
	"PageUp":     kb.PageUp,
	"PageDown":   kb.PageDown,
}

// handleInput executes one panel interaction in ITS tab (each panel's input
// stream is bound to the tab it views, so the user can use any tab regardless
// of which one the agent holds active). It serializes with agent actions under
// the same lock and resets the idle clock (a user mid-login keeps the browser
// alive). Positions arrive normalized; they scale against the CSS visual
// viewport at dispatch time, so clicks land where the user saw them regardless
// of frame scaling. Best-effort: a failed click is a no-op the user retries.
func (b *Browser) handleInput(t *tab, ev InputEvent) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if _, ok := b.tabs[t.id]; !ok {
		return // tab closed under the panel
	}
	_ = b.runOnLocked(context.Background(), t, chromedp.ActionFunc(func(ctx context.Context) error {
		switch ev.T {
		case "click", "wheel":
			_, _, _, _, cssVisual, _, err := page.GetLayoutMetrics().Do(ctx)
			if err != nil || cssVisual == nil {
				return err
			}
			x := ev.X * cssVisual.ClientWidth
			y := ev.Y * cssVisual.ClientHeight
			if ev.T == "click" {
				return chromedp.MouseClickXY(x, y).Do(ctx)
			}
			return input.DispatchMouseEvent(input.MouseWheel, x, y).
				WithDeltaX(0).WithDeltaY(ev.DY).Do(ctx)
		case "key":
			if k, ok := specialKeys[ev.Key]; ok {
				return chromedp.KeyEvent(k).Do(ctx)
			}
			return nil
		case "text":
			if ev.S != "" {
				return input.InsertText(ev.S).Do(ctx)
			}
			return nil
		case "nav":
			// The panel's address bar: a typed address or search, normalized
			// the way a browser's omnibox would.
			if u := normalizeAddress(ev.S); u != "" {
				return chromedp.Navigate(u).Do(ctx)
			}
			return nil
		case "back":
			// The panel's back/forward/reload controls. NavigateBack/Forward
			// return a harmless error at the ends of history (nothing to go
			// to); this is best-effort, so that no-op is swallowed.
			return chromedp.NavigateBack().Do(ctx)
		case "forward":
			return chromedp.NavigateForward().Do(ctx)
		case "reload":
			return chromedp.Reload().Do(ctx)
		}
		return nil
	}))
}

// normalizeAddress turns omnibox input into a navigable URL: full URLs pass
// through, something domain-shaped gets https://, and anything else (spaces, no
// dot) becomes a web search.
func normalizeAddress(s string) string {
	s = strings.TrimSpace(s)
	switch {
	case s == "":
		return ""
	case strings.Contains(s, "://"):
		return s
	case strings.ContainsAny(s, " ") || !strings.Contains(s, "."):
		return "https://www.google.com/search?q=" + url.QueryEscape(s)
	default:
		return "https://" + s
	}
}

// PageState is the cheap text view the model reads after acting: the document
// title, the current URL, and a bounded slice of visible text.
type PageState struct {
	Title string
	URL   string
	Text  string
}

// textCap bounds the innerText returned with a page state, so a long page does
// not flood the model's context. The agent screenshots when it needs to see.
const textCap = 4000

// Navigate loads url in the active tab, waits for the load event, and returns
// the resulting page state.
func (b *Browser) Navigate(ctx context.Context, url string) (PageState, error) {
	var st PageState
	err := b.run(ctx,
		chromedp.Navigate(url),
		chromedp.Title(&st.Title),
		chromedp.Location(&st.URL),
		chromedp.Evaluate(innerTextJS, &st.Text),
	)
	st.Text = clip(st.Text, textCap)
	return st, err
}

// Read returns the active tab's page state — title, URL, and visible text of
// the whole page or, when selector is set, of the matched element.
func (b *Browser) Read(ctx context.Context, selector string) (PageState, error) {
	var st PageState
	expr := innerTextJS
	if strings.TrimSpace(selector) != "" {
		expr = fmt.Sprintf("(document.querySelector(%q)||{}).innerText||''", selector)
	}
	err := b.run(ctx,
		chromedp.Title(&st.Title),
		chromedp.Location(&st.URL),
		chromedp.Evaluate(expr, &st.Text),
	)
	st.Text = clip(st.Text, textCap)
	return st, err
}

// Screenshot captures the active tab: the visible viewport by default, the
// full scrollable page when fullPage is set, or a single element when selector
// is set. Viewport and element captures are PNG; fullPage is JPEG (chromedp
// encodes as JPEG for any quality < 100), so callers must sniff the real type
// rather than assume PNG.
func (b *Browser) Screenshot(ctx context.Context, selector string, fullPage bool) ([]byte, error) {
	var buf []byte
	var action chromedp.Action
	switch {
	case strings.TrimSpace(selector) != "":
		action = chromedp.Screenshot(selector, &buf, chromedp.NodeVisible, chromedp.ByQuery)
	case fullPage:
		action = chromedp.FullScreenshot(&buf, 90)
	default:
		action = chromedp.CaptureScreenshot(&buf)
	}
	if err := b.run(ctx, action); err != nil {
		return nil, err
	}
	return buf, nil
}

// Click clicks the first element matching selector in the active tab (waiting
// for it to be visible first).
func (b *Browser) Click(ctx context.Context, selector string) error {
	return b.run(ctx, chromedp.Click(selector, chromedp.NodeVisible, chromedp.ByQuery))
}

// Type focuses the element matching selector in the active tab and types text
// into it, optionally pressing Enter to submit.
func (b *Browser) Type(ctx context.Context, selector, text string, submit bool) error {
	value := text
	if submit {
		value += "\r"
	}
	return b.run(ctx, chromedp.SendKeys(selector, value, chromedp.ByQuery))
}

// Viewport resizes the active tab's emulated viewport, for responsive-design
// checks.
func (b *Browser) Viewport(ctx context.Context, width, height int) error {
	return b.run(ctx, chromedp.EmulateViewport(int64(width), int64(height)))
}

// Eval runs a JavaScript expression in the active tab and returns its
// JSON-encoded result, for power use the dedicated actions don't cover.
func (b *Browser) Eval(ctx context.Context, expression string) (string, error) {
	var res json.RawMessage
	if err := b.run(ctx, chromedp.Evaluate(expression, &res)); err != nil {
		return "", err
	}
	return string(res), nil
}

// innerTextJS reads the document's visible text, empty-string-safe before the
// body exists.
const innerTextJS = "document.body ? document.body.innerText : ''"

// clip truncates s to at most n runes, appending a notice when it cut.
func clip(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "\n\n[…truncated; use a selector or screenshot for more]"
}
