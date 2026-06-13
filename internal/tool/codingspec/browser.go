package codingspec

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	"github.com/unarbos/arbos/internal/browser"
	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// BrowserArgs are the arguments to browser. One tool, one action enum: a single
// stateful handle to a real browser, dispatched by Action, rather than a family
// of tools that would each re-explain the same browser.
type BrowserArgs struct {
	Action     string `json:"action" desc:"What to do: navigate, screenshot, read, click, type, viewport, eval, tabs, new_tab, switch_tab, or close_tab."`
	Tab        string `json:"tab,omitempty" desc:"Tab id (from tabs) for switch_tab / close_tab."`
	URL        string `json:"url,omitempty" desc:"URL to open (action=navigate)."`
	Selector   string `json:"selector,omitempty" desc:"CSS selector to target (screenshot of one element, read, click, type)."`
	Text       string `json:"text,omitempty" desc:"Text to type into the selected field (action=type)."`
	Submit     bool   `json:"submit,omitempty" desc:"Press Enter after typing (action=type)."`
	FullPage   bool   `json:"fullPage,omitempty" desc:"Capture the whole scrollable page, not just the viewport (action=screenshot)."`
	Width      int    `json:"width,omitempty" desc:"Viewport width in pixels (action=viewport)."`
	Height     int    `json:"height,omitempty" desc:"Viewport height in pixels (action=viewport)."`
	Expression string `json:"expression,omitempty" desc:"JavaScript expression to evaluate in the page (action=eval)."`
}

const browserDescription = "Drive a real browser (Chromium) to render and act on pages a plain fetch can't: a JavaScript-rendered site, the localhost dev server, a click, a form fill. " +
	"Each tab live-streams into a \"Browser\" panel beside the chat, so the user watches you browse in real time — to show the user a page, just navigate to it. " +
	"Panels are interactive: the user can click, scroll, and type directly in them. " +
	"The profile is persistent, so sites stay logged in across sessions; when you need access the profile doesn't have yet, navigate to the sign-in page and ask the user to log in right in the panel, then continue. " +
	"The browser has tabs; your actions hit the ACTIVE tab. The user may be using other tabs — prefer your own tab over navigating away from a page the user is on (tabs lists them, new_tab opens yours). " +
	"Actions: " +
	"navigate (open a url), " +
	"screenshot (capture the viewport — or fullPage, or one element by selector — returned as an image you can see; use this to look at a design and iterate on it), " +
	"read (the page's visible text, whole page or one selector), " +
	"click (a selector), " +
	"type (text into a selector, optionally submit), " +
	"viewport (resize for responsive checks), " +
	"eval (run a JavaScript expression), " +
	"tabs (list open tabs), new_tab (open + switch to a fresh tab), switch_tab (tab=id), close_tab (tab=id). " +
	"Tabs keep their pages between calls within a session."

// browserSpec builds the browser tool. Each result carries a "screencast"
// surface naming the ACTIVE tab's live stream, so the panel that auto-opens
// beside the chat follows the tab the agent is working in (a new_tab opens a
// new panel; the user's other tabs keep their own).
func browserSpec(b *browser.Browser) tool.Spec {
	return tool.NewRichSpec("browser", browserDescription, false,
		func(ctx context.Context, a BrowserArgs) (tool.Result, error) {
			res, err := dispatchBrowser(ctx, b, a)
			if err != nil {
				return tool.Result{}, err
			}
			// Attach the live panel reference so arbos opens/refreshes it.
			if stream := b.ActiveStream(); stream != "" {
				if live, err := json.Marshal(surfaceDetails{Surface: surface{
					Kind: "screencast", Path: stream, Title: "Browser " + tabIDOf(stream),
				}}); err == nil {
					res.Details = live
				}
			}
			return res, nil
		})
}

// tabIDOf extracts the tab id from a stream ("<workspace>/<tab>").
func tabIDOf(stream string) string {
	if i := strings.LastIndex(stream, "/"); i >= 0 {
		return stream[i+1:]
	}
	return stream
}

func dispatchBrowser(ctx context.Context, b *browser.Browser, a BrowserArgs) (tool.Result, error) {
	switch strings.ToLower(strings.TrimSpace(a.Action)) {
	case "navigate":
		if strings.TrimSpace(a.URL) == "" {
			return tool.Result{}, fmt.Errorf("browser: navigate requires a url")
		}
		st, err := b.Navigate(ctx, a.URL)
		if err != nil {
			return tool.Result{}, fmt.Errorf("browser navigate: %w", err)
		}
		return tool.Result{Content: pageStateText(st)}, nil

	case "read":
		st, err := b.Read(ctx, a.Selector)
		if err != nil {
			return tool.Result{}, fmt.Errorf("browser read: %w", err)
		}
		return tool.Result{Content: pageStateText(st)}, nil

	case "screenshot":
		png, err := b.Screenshot(ctx, a.Selector, a.FullPage)
		if err != nil {
			return tool.Result{}, fmt.Errorf("browser screenshot: %w", err)
		}
		return tool.Result{
			Content: fmt.Sprintf("Screenshot captured (%s, %s).", screenshotScope(a), FormatSize(len(png))),
			Blocks: []core.ContentBlock{{
				Type:  core.BlockImage,
				Image: &core.ImageData{Data: base64.StdEncoding.EncodeToString(png), MimeType: screenshotMime(png)},
			}},
		}, nil

	case "click":
		if strings.TrimSpace(a.Selector) == "" {
			return tool.Result{}, fmt.Errorf("browser: click requires a selector")
		}
		if err := b.Click(ctx, a.Selector); err != nil {
			return tool.Result{}, fmt.Errorf("browser click: %w", err)
		}
		return tool.Result{Content: fmt.Sprintf("Clicked %s.", a.Selector)}, nil

	case "type":
		if strings.TrimSpace(a.Selector) == "" {
			return tool.Result{}, fmt.Errorf("browser: type requires a selector")
		}
		if err := b.Type(ctx, a.Selector, a.Text, a.Submit); err != nil {
			return tool.Result{}, fmt.Errorf("browser type: %w", err)
		}
		submitted := ""
		if a.Submit {
			submitted = " and submitted"
		}
		return tool.Result{Content: fmt.Sprintf("Typed into %s%s.", a.Selector, submitted)}, nil

	case "viewport":
		if a.Width <= 0 || a.Height <= 0 {
			return tool.Result{}, fmt.Errorf("browser: viewport requires positive width and height")
		}
		if err := b.Viewport(ctx, a.Width, a.Height); err != nil {
			return tool.Result{}, fmt.Errorf("browser viewport: %w", err)
		}
		return tool.Result{Content: fmt.Sprintf("Viewport set to %d×%d.", a.Width, a.Height)}, nil

	case "eval":
		if strings.TrimSpace(a.Expression) == "" {
			return tool.Result{}, fmt.Errorf("browser: eval requires an expression")
		}
		res, err := b.Eval(ctx, a.Expression)
		if err != nil {
			return tool.Result{}, fmt.Errorf("browser eval: %w", err)
		}
		return tool.Result{Content: res}, nil

	case "tabs":
		tabs := b.Tabs()
		if len(tabs) == 0 {
			return tool.Result{Content: "No tabs open (the browser is not running; navigate or new_tab opens one)."}, nil
		}
		var sb strings.Builder
		for _, t := range tabs {
			marker := " "
			if t.Active {
				marker = "*"
			}
			u := t.URL
			if u == "" {
				u = "(blank)"
			}
			fmt.Fprintf(&sb, "%s %s  %s\n", marker, t.ID, u)
		}
		sb.WriteString("(* = your active tab)")
		return tool.Result{Content: sb.String()}, nil

	case "new_tab", "newtab":
		t, err := b.NewTab(ctx)
		if err != nil {
			return tool.Result{}, fmt.Errorf("browser new_tab: %w", err)
		}
		return tool.Result{Content: fmt.Sprintf("Opened tab %s and switched to it.", t.ID)}, nil

	case "switch_tab":
		if strings.TrimSpace(a.Tab) == "" {
			return tool.Result{}, fmt.Errorf("browser: switch_tab requires tab (an id from tabs)")
		}
		if err := b.SwitchTab(ctx, a.Tab); err != nil {
			return tool.Result{}, fmt.Errorf("browser switch_tab: %w", err)
		}
		return tool.Result{Content: fmt.Sprintf("Switched to tab %s.", a.Tab)}, nil

	case "close_tab":
		if strings.TrimSpace(a.Tab) == "" {
			return tool.Result{}, fmt.Errorf("browser: close_tab requires tab (an id from tabs)")
		}
		if err := b.CloseTab(ctx, a.Tab); err != nil {
			return tool.Result{}, fmt.Errorf("browser close_tab: %w", err)
		}
		return tool.Result{Content: fmt.Sprintf("Closed tab %s.", a.Tab)}, nil

	case "":
		return tool.Result{}, fmt.Errorf("browser: action is required (navigate, screenshot, read, click, type, viewport, eval, tabs, new_tab, switch_tab, close_tab)")
	default:
		return tool.Result{}, fmt.Errorf("browser: unknown action %q", a.Action)
	}
}

// pageStateText renders a page state for the model: title and URL, then the
// visible text.
func pageStateText(st browser.PageState) string {
	var b strings.Builder
	if st.Title != "" {
		fmt.Fprintf(&b, "# %s\n", st.Title)
	}
	if st.URL != "" {
		fmt.Fprintf(&b, "%s\n", st.URL)
	}
	if b.Len() > 0 && st.Text != "" {
		b.WriteString("\n")
	}
	b.WriteString(st.Text)
	out := strings.TrimSpace(b.String())
	if out == "" {
		return "(page is empty)"
	}
	return out
}

// screenshotMime sniffs the real image type from the capture's magic bytes.
// chromedp returns PNG for viewport/element captures but JPEG for fullPage
// (it encodes as JPEG whenever quality != 100), so a hardcoded "image/png"
// would mislabel fullPage shots — and Anthropic rejects the request (400) when
// the declared media_type doesn't match the bytes. Detecting from the content
// keeps the label honest regardless of capture mode.
func screenshotMime(img []byte) string {
	switch ct := http.DetectContentType(img); ct {
	case "image/png", "image/jpeg", "image/gif", "image/webp":
		return ct
	default:
		return "image/png"
	}
}

// screenshotScope labels what a screenshot captured, for the result text.
func screenshotScope(a BrowserArgs) string {
	switch {
	case strings.TrimSpace(a.Selector) != "":
		return "element " + a.Selector
	case a.FullPage:
		return "full page"
	default:
		return "viewport"
	}
}
