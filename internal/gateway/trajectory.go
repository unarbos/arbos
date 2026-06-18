// Trajectory sharing: a session's full debug log rendered as a single,
// self-contained page. Where ScopeSession hands a guest the live chat behind
// the real UI, ScopeTrajectory hands them a static reproduction served
// straight from the event log — every user and assistant message (reasoning
// included), every tool call with its verbatim arguments, every tool result,
// the injected context, the turn config the model actually ran with, and the
// token totals. It is the one-click answer to "send me the trajectory so I can
// debug it": the same thing `arbos export` writes as JSONL, rendered for a
// human to read at a link. The page is sandboxed like any shared file artifact
// and makes no outbound requests.

package gateway

import (
	"bytes"
	"encoding/json"
	"fmt"
	"html"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/share"
	"github.com/unarbos/arbos/internal/trajectory"
)

// serveTrajectory serves the granted session's trajectory. The same link
// answers two audiences by content negotiation: a machine (the priority — an
// agent auditing "was this the behavior we wanted?") gets the lossless event
// JSONL; a browser gets the rendered HTML view. Every format is passed through
// the secret redactor before it leaves the box: a shared link is the one path
// that ships log content externally, so it gets the same best-effort scrub the
// logging layer applies (the local `arbos export` stays full-fidelity). A grant
// only mints against a session that existed, but the log can still be
// unreadable (a since-deleted store, an I/O error); that is the dead-link page,
// the same as any other share that can no longer resolve.
func (s *Server) serveTrajectory(w http.ResponseWriter, r *http.Request, g share.Grant) {
	id := core.SessionID(g.Scope.Ref)
	sess, err := s.Store.Get(r.Context(), id)
	if err != nil {
		shareGone(w)
		return
	}
	events, err := s.Store.Events(r.Context(), id)
	if err != nil {
		shareGone(w)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")

	// No default: trajectoryFormat is the only producer of trajFormat and can
	// return only these three, so exhaustive guards the closed set at compile
	// time — adding a default here would defeat that check.
	switch trajectoryFormat(r) {
	case fmtHTML:
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		sandboxArtifact(w)
		_, _ = io.WriteString(w, obs.RedactString(renderTrajectory(sess, events, r.URL.Path)))
	case fmtMessages:
		writeRedactedNDJSON(w, func(enc *json.Encoder) error { return trajectory.WriteMessages(enc, id, events, sess.Model) })
	case fmtJSONL:
		writeRedactedNDJSON(w, func(enc *json.Encoder) error { return trajectory.WriteEvents(enc, id, events) })
	}
}

// writeRedactedNDJSON renders trajectory NDJSON into a buffer, runs the secret
// redactor over it, then writes the result — the scrub only the share path
// applies. Buffering (rather than streaming straight to the response) is the
// price of redacting before the bytes leave; trajectories are bounded, and this
// is the share boundary, not a hot loop. Redacting the serialized text is safe
// for JSON: no secretPattern branch can match a `"` (every token class excludes
// it), so a hit stays inside one string value, and the [REDACTED] mark has no
// JSON-special characters — structure is preserved. A generation error is
// refused rather than written: buffering lets us check before any byte ships,
// so a failed encode returns 500 instead of a truncated body under 200.
func writeRedactedNDJSON(w http.ResponseWriter, gen func(*json.Encoder) error) {
	var buf bytes.Buffer
	if err := gen(trajectory.NewEncoder(&buf)); err != nil {
		http.Error(w, "trajectory unavailable", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/x-ndjson; charset=utf-8")
	_, _ = io.WriteString(w, obs.RedactString(buf.String()))
}

// trajFormat is the trajectory representation a request resolves to. The three
// values are the single source of truth shared by the format parser, the
// response switch, and the HTML view's alternate-format links, so a rename or a
// new format can't leave a dangling ?format= the parser no longer honors.
type trajFormat string

const (
	fmtHTML     trajFormat = "html"
	fmtMessages trajFormat = "messages"
	fmtJSONL    trajFormat = "jsonl"
)

// trajectoryFormat picks the representation for a trajectory request. An
// explicit ?format= wins; otherwise a browser (Accept: text/html) gets HTML and
// everything else — curl, an SDK, a fetching agent — gets the lossless JSONL,
// so the machine path is the default without a human ever choosing a flag.
func trajectoryFormat(r *http.Request) trajFormat {
	switch r.URL.Query().Get("format") {
	case string(fmtHTML):
		return fmtHTML
	case string(fmtMessages):
		return fmtMessages
	case string(fmtJSONL), "json", "events":
		return fmtJSONL
	}
	if strings.Contains(r.Header.Get("Accept"), "text/html") {
		return fmtHTML
	}
	return fmtJSONL
}

// renderTrajectory builds the self-contained HTML for one session's log. It
// walks the events in order, folding usage into a running total and rendering
// every other kind into the timeline. Everything user-controlled is escaped;
// the page ships its own styling and runs one tiny inline script (expand/
// collapse all), within the artifact sandbox.
func renderTrajectory(sess core.Session, events []core.Event, selfPath string) string {
	var b strings.Builder
	var totalPrompt, totalCompletion, totalTokens int
	var turns int

	body := &strings.Builder{}
	for i := range events {
		ev := events[i]
		// Switch on the Kind discriminator, not the Go type: the `exhaustive`
		// linter checks EventKind switches, so a new payload kind fails CI here
		// until it is given a row — a type switch would let it slip silently
		// into the unknown-payload fallback. The trailing renderUnknown covers
		// kinds from newer writers (ADR-0010: surface, never drop).
		switch ev.Payload.Kind() {
		case core.EventUserMessage, core.EventAssistantMessage:
			m := ev.Payload.(core.MessagePayload).Message
			if m.Role == core.RoleUser {
				turns++
				renderUserMessage(body, ev, m)
			} else {
				renderAssistantMessage(body, ev, m)
			}
			continue
		case core.EventToolResult:
			renderToolResult(body, ev, ev.Payload.(core.ToolResultPayload).Result)
			continue
		case core.EventContext:
			renderContext(body, ev, ev.Payload.(core.ContextPayload).Segments)
			continue
		case core.EventConfig:
			renderConfig(body, ev, ev.Payload.(core.ConfigPayload))
			continue
		case core.EventCompressed:
			renderCompression(body, ev, ev.Payload.(core.CompressionPayload))
			continue
		case core.EventInterrupted:
			renderInterrupt(body, ev, ev.Payload.(core.InterruptPayload).Reason)
			continue
		case core.EventUsage:
			u := ev.Payload.(core.UsagePayload).Usage
			totalPrompt += u.PromptTokens
			totalCompletion += u.CompletionTokens
			totalTokens += u.TotalTokens
			continue
		case core.EventChatNote:
			// Human-to-human side chat is private coordination, not part of the
			// agent's auditable trajectory — skip it (matches WriteEvents).
			continue
		case core.EventBranchAnchor:
			// Anchor/merge bookkeeping with no conversational content — skip it
			// (matches the projection in core.project.go).
			continue
		}
		renderUnknown(body, ev)
	}

	model := sess.Model
	if cfg, ok := core.LatestConfig(events); ok && cfg.Model != "" {
		model = cfg.Model
	}

	b.WriteString(trajectoryHead)
	b.WriteString(`<div class="wrap">`)
	b.WriteString(`<header class="hdr">`)
	b.WriteString(`<div class="title">Trajectory</div>`)
	b.WriteString(`<div class="sub">` + esc(string(sess.ID)) + `</div>`)
	b.WriteString(`<div class="meta">`)
	metaItem(&b, "model", model)
	metaItem(&b, "status", string(sess.Status))
	metaItem(&b, "events", fmt.Sprintf("%d", len(events)))
	metaItem(&b, "turns", fmt.Sprintf("%d", turns))
	if !sess.CreatedAt.IsZero() {
		metaItem(&b, "started", sess.CreatedAt.UTC().Format("2006-01-02 15:04 UTC"))
	}
	if totalTokens > 0 || totalPrompt > 0 {
		metaItem(&b, "tokens", fmt.Sprintf("%s prompt + %s completion = %s total",
			commas(totalPrompt), commas(totalCompletion), commas(max(totalTokens, totalPrompt+totalCompletion))))
	}
	b.WriteString(`</div>`)
	b.WriteString(`<div class="actions">`)
	b.WriteString(`<button class="toggleall" onclick="for(const d of document.querySelectorAll('details'))d.open=!d.open">expand / collapse all</button>`)
	if selfPath != "" {
		// The machine path is the point of this artifact: a one-click jump to
		// the lossless event JSONL an agent reads back to audit the run. Same
		// bytes as `arbos export`.
		jsonHref := esc(selfPath + "?format=" + string(fmtJSONL))
		msgHref := esc(selfPath + "?format=" + string(fmtMessages))
		b.WriteString(`<a class="raw" href="` + jsonHref + `">event log (JSONL)</a>`)
		b.WriteString(`<a class="raw" href="` + msgHref + `">messages</a>`)
	}
	b.WriteString(`</div>`)
	b.WriteString(`</header>`)
	b.WriteString(`<main class="log">`)
	b.WriteString(body.String())
	b.WriteString(`</main>`)
	b.WriteString(`<footer class="ftr">Rendered by arbos · ` + time.Now().UTC().Format("2006-01-02 15:04 UTC") + `</footer>`)
	b.WriteString(`</div>`)
	return b.String()
}

func metaItem(b *strings.Builder, k, v string) {
	if v == "" {
		return
	}
	b.WriteString(`<span class="mi"><b>` + esc(k) + `</b> ` + esc(v) + `</span>`)
}

func renderUserMessage(b *strings.Builder, ev core.Event, m core.Message) {
	b.WriteString(`<section class="ev user">`)
	// A named multi-party guest shows by name; otherwise fall back to the door
	// marker (e.g. "telegram") the snapshot has always shown.
	tag := m.Author
	if tag == "" {
		tag = m.Origin
	}
	rowHead(b, ev, "user", tag)
	b.WriteString(`<pre class="text">` + esc(m.Content) + `</pre>`)
	renderBlocks(b, m.Parts)
	b.WriteString(`</section>`)
}

func renderAssistantMessage(b *strings.Builder, ev core.Event, m core.Message) {
	b.WriteString(`<section class="ev assistant">`)
	rowHead(b, ev, "assistant", "")
	if strings.TrimSpace(m.Reasoning) != "" {
		b.WriteString(`<details class="reasoning"><summary>reasoning</summary><pre class="text dim">` + esc(m.Reasoning) + `</pre></details>`)
	}
	if strings.TrimSpace(m.Content) != "" {
		b.WriteString(`<pre class="text">` + esc(m.Content) + `</pre>`)
	}
	renderBlocks(b, m.Parts)
	for _, tc := range m.ToolCalls {
		b.WriteString(`<div class="call">`)
		b.WriteString(`<div class="callname">→ ` + esc(tc.Name))
		if tc.ID != "" {
			b.WriteString(` <span class="cid">` + esc(tc.ID) + `</span>`)
		}
		b.WriteString(`</div>`)
		if args := prettyJSON(tc.Args); args != "" {
			b.WriteString(`<pre class="args">` + esc(args) + `</pre>`)
		}
		b.WriteString(`</div>`)
	}
	b.WriteString(`</section>`)
}

func renderToolResult(b *strings.Builder, ev core.Event, res core.ToolResult) {
	cls := "ev tool"
	if res.IsError {
		cls += " err"
	}
	b.WriteString(`<section class="` + cls + `">`)
	label := "tool result"
	if res.IsError {
		label = "tool error"
	}
	rowHead(b, ev, label, res.CallID)
	if res.Content != "" {
		b.WriteString(`<pre class="text out">` + esc(res.Content) + `</pre>`)
	}
	renderBlocks(b, res.Blocks)
	if details := prettyJSON(res.Details); details != "" {
		b.WriteString(`<details class="extra"><summary>details</summary><pre class="args">` + esc(details) + `</pre></details>`)
	}
	b.WriteString(`</section>`)
}

func renderContext(b *strings.Builder, ev core.Event, segs []core.Segment) {
	b.WriteString(`<section class="ev ctx">`)
	b.WriteString(`<details><summary>` + rowLabel(ev, fmt.Sprintf("injected context · %d segment(s)", len(segs))) + `</summary>`)
	for _, sg := range segs {
		b.WriteString(`<div class="seg"><div class="segsrc">` + esc(sg.Source) + `</div><pre class="text dim">` + esc(sg.Content) + `</pre></div>`)
	}
	b.WriteString(`</details></section>`)
}

func renderConfig(b *strings.Builder, ev core.Event, p core.ConfigPayload) {
	b.WriteString(`<section class="ev cfg">`)
	b.WriteString(`<details><summary>` + rowLabel(ev, "turn config") + `</summary>`)
	b.WriteString(`<div class="kv"><b>model</b> ` + esc(p.Model) + `</div>`)
	toggles := []string{}
	if p.WebSearch {
		toggles = append(toggles, "web_search")
	}
	if p.WebFetch {
		toggles = append(toggles, "web_fetch")
	}
	if p.ImageGen {
		toggles = append(toggles, "image_gen")
	}
	if len(toggles) > 0 {
		b.WriteString(`<div class="kv"><b>enabled</b> ` + esc(strings.Join(toggles, ", ")) + `</div>`)
	}
	if names := toolNames(p.Tools); names != "" {
		b.WriteString(`<div class="kv"><b>tools</b> ` + esc(names) + `</div>`)
	}
	if strings.TrimSpace(p.SystemPrompt) != "" {
		b.WriteString(`<details class="extra"><summary>system prompt</summary><pre class="text dim">` + esc(p.SystemPrompt) + `</pre></details>`)
	}
	b.WriteString(`</details></section>`)
}

func renderCompression(b *strings.Builder, ev core.Event, p core.CompressionPayload) {
	b.WriteString(`<section class="ev compressed">`)
	b.WriteString(`<details><summary>` + rowLabel(ev, fmt.Sprintf("compressed seq %d–%d", p.ReplacedSeqLo, p.ReplacedSeqHi)) + `</summary>`)
	b.WriteString(`<pre class="text dim">` + esc(p.Summary) + `</pre>`)
	b.WriteString(`</details></section>`)
}

func renderInterrupt(b *strings.Builder, ev core.Event, reason string) {
	if reason == "" {
		reason = "turn interrupted"
	}
	b.WriteString(`<section class="ev interrupted"><div class="marker">⊘ ` + rowLabel(ev, reason) + `</div></section>`)
}

func renderUnknown(b *strings.Builder, ev core.Event) {
	raw, err := core.EncodePayload(ev.Payload)
	if err != nil {
		return
	}
	b.WriteString(`<section class="ev cfg"><details><summary>` + rowLabel(ev, string(ev.Payload.Kind())) + `</summary><pre class="args">` + esc(prettyJSON(raw)) + `</pre></details></section>`)
}

// renderBlocks renders non-text content (images inline as data URIs, files as
// a labelled note). Images already live in the log, so showing them keeps a
// visual trajectory visual; a sandboxed data URI makes no network request.
func renderBlocks(b *strings.Builder, blocks []core.ContentBlock) {
	for _, blk := range blocks {
		switch blk.Type {
		case core.BlockImage:
			if blk.Image != nil && blk.Image.Data != "" {
				mime := blk.Image.MimeType
				if mime == "" {
					mime = "image/png"
				}
				b.WriteString(`<img class="img" alt="image" src="data:` + esc(mime) + `;base64,` + esc(blk.Image.Data) + `">`)
			}
		case core.BlockFile:
			name := "file"
			if blk.File != nil && blk.File.Name != "" {
				name = blk.File.Name
			}
			b.WriteString(`<div class="kv"><b>file</b> ` + esc(name) + `</div>`)
		case core.BlockText:
			if blk.Text != "" {
				b.WriteString(`<pre class="text">` + esc(blk.Text) + `</pre>`)
			}
		}
	}
}

// rowHead writes the per-event header line: a kind tag, the sequence number,
// and an optional right-aligned note (a message origin, a tool call id).
func rowHead(b *strings.Builder, ev core.Event, kind, note string) {
	b.WriteString(`<div class="row"><span class="tag">` + esc(kind) + `</span>`)
	b.WriteString(`<span class="seq">#` + fmt.Sprintf("%d", ev.Seq) + `</span>`)
	if note != "" {
		b.WriteString(`<span class="note">` + esc(note) + `</span>`)
	}
	b.WriteString(`</div>`)
}

// rowLabel is rowHead's inline form for a <summary>, returning escaped HTML.
func rowLabel(ev core.Event, label string) string {
	return `<span class="seq">#` + fmt.Sprintf("%d", ev.Seq) + `</span> ` + esc(label)
}

func toolNames(tools []core.ToolSchema) string {
	if len(tools) == 0 {
		return ""
	}
	names := make([]string, 0, len(tools))
	for _, t := range tools {
		names = append(names, t.Name)
	}
	return strings.Join(names, ", ")
}

func prettyJSON(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var buf bytes.Buffer
	if err := json.Indent(&buf, raw, "", "  "); err != nil {
		return string(raw)
	}
	return buf.String()
}

func esc(s string) string { return html.EscapeString(s) }

// commas formats an int with thousands separators for the token totals.
func commas(n int) string {
	s := fmt.Sprintf("%d", n)
	if n < 0 {
		return s
	}
	var out []byte
	for i, c := range []byte(s) {
		if i > 0 && (len(s)-i)%3 == 0 {
			out = append(out, ',')
		}
		out = append(out, c)
	}
	return string(out)
}

// trajectoryHead is the page shell: doctype, meta, and the self-contained
// styling. The palette tracks the login page's so a shared trajectory looks
// like part of arbos. No external requests — fonts are system, there are no
// stylesheets or scripts to fetch.
const trajectoryHead = `<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>arbos — trajectory</title>
<style>
  :root {
    --canvas:#2c262a; --panel:#312b2f; --line:#3e373c;
    --text:#d6d0d4; --bright:#ece7ea; --muted:#968f94; --faint:#6e676c;
    --accent:#85aab3; --user:#8fb3a0; --err:#c98a8a;
  }
  * { box-sizing: border-box; }
  body {
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Inter,Roboto,sans-serif;
    font-size:13px; line-height:1.6; color-scheme:dark;
    -webkit-font-smoothing:antialiased;
    background:var(--canvas); color:var(--text); margin:0;
  }
  ::selection { background: color-mix(in srgb, var(--accent) 28%, transparent); }
  .wrap { max-width: 56rem; margin: 0 auto; padding: 1.4rem 1rem 4rem; }
  .hdr { border-bottom:1px solid var(--line); padding-bottom:1rem; margin-bottom:1rem; }
  .title { font-size:16px; font-weight:600; color:var(--bright); }
  .sub { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:11px; color:var(--muted); margin-top:.15rem; word-break:break-all; }
  .meta { display:flex; flex-wrap:wrap; gap:.35rem .9rem; margin-top:.7rem; }
  .mi { font-size:11.5px; color:var(--muted); }
  .mi b { color:var(--faint); font-weight:600; margin-right:.25rem; text-transform:uppercase; letter-spacing:.04em; font-size:10px; }
  .actions { display:flex; flex-wrap:wrap; align-items:center; gap:.5rem; margin-top:.8rem; }
  .toggleall { background:var(--panel); border:1px solid var(--line); color:var(--muted);
    padding:.3rem .6rem; border-radius:6px; font:inherit; font-size:11.5px; cursor:pointer; }
  .toggleall:hover { color:var(--bright); }
  a.raw { color:var(--accent); text-decoration:none; font-size:11.5px;
    border:1px solid color-mix(in srgb, var(--accent) 35%, var(--line)); border-radius:6px; padding:.3rem .6rem; }
  a.raw:hover { background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .log { display:flex; flex-direction:column; gap:.5rem; }
  .ev { border:1px solid var(--line); border-radius:8px; padding:.6rem .8rem; background:var(--panel); }
  .ev.user { border-color: color-mix(in srgb, var(--user) 40%, var(--line)); }
  .ev.assistant { background: color-mix(in srgb, var(--accent) 7%, var(--panel)); }
  .ev.tool.err { border-color: color-mix(in srgb, var(--err) 45%, var(--line)); }
  .ev.ctx, .ev.cfg, .ev.compressed { background:transparent; border-style:dashed; }
  .row { display:flex; align-items:center; gap:.5rem; margin-bottom:.3rem; }
  .tag { font-size:10px; font-weight:700; letter-spacing:.05em; text-transform:uppercase;
    color:var(--canvas); background:var(--muted); border-radius:4px; padding:.05rem .35rem; }
  .ev.user .tag { background:var(--user); }
  .ev.assistant .tag { background:var(--accent); }
  .ev.tool .tag { background:var(--btn,#b0aaae); }
  .ev.tool.err .tag { background:var(--err); }
  .seq { font-family:ui-monospace,monospace; font-size:10.5px; color:var(--faint); }
  .note { font-family:ui-monospace,monospace; font-size:10.5px; color:var(--muted); margin-left:auto; word-break:break-all; }
  pre.text, pre.args { margin:.2rem 0 0; white-space:pre-wrap; word-break:break-word;
    font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:12px; }
  pre.text { font-family:inherit; font-size:13px; }
  pre.out, pre.args { background:var(--canvas); border:1px solid var(--line); border-radius:6px;
    padding:.5rem .6rem; max-height:24rem; overflow:auto; }
  .dim { color:var(--muted); }
  .call { margin-top:.45rem; }
  .callname { font-family:ui-monospace,monospace; font-size:12px; color:var(--bright); }
  .cid { color:var(--faint); font-size:10.5px; }
  details { margin-top:.35rem; }
  summary { cursor:pointer; color:var(--muted); font-size:12px; }
  summary:hover { color:var(--bright); }
  details.extra > summary, details.reasoning > summary { font-size:11.5px; }
  .seg { margin-top:.4rem; }
  .segsrc { font-size:10px; text-transform:uppercase; letter-spacing:.05em; color:var(--faint); }
  .kv { font-size:12px; margin-top:.2rem; }
  .kv b { color:var(--faint); font-weight:600; margin-right:.35rem; }
  .marker { color:var(--muted); font-size:12px; }
  .img { max-width:100%; border:1px solid var(--line); border-radius:6px; margin-top:.4rem; }
  .ftr { margin-top:1.5rem; color:var(--faint); font-size:11px; text-align:center; }
</style>
`
