---
description: Research a question across many sources and write a cited, shareable report
argument-hint: [the question or topic to research]
---

Research the topic below the way a good analyst would, then write it up as a long-form, cited report — a "blog". This is deep research: plan the inquiry, gather from many real sources, reason as you go, and synthesize a structured document a reader can trust and share.

<topic>
$ARGUMENTS
</topic>

If the topic block is empty, research the subject currently under discussion in this conversation; if there is none, ask for one in a single line and stop.

## What a blog is

A single long-form markdown report written to `blog/<short-slug>-<yyyy-mm-dd>.md` — a structured, readable document with headings, prose, and a **Sources** section, where every non-obvious claim traces to a real source you actually read. It is text-based on purpose: it renders in the doc panel (preview + source), edits inline, and shares as a clean reading link. Never invent facts, quotes, numbers, or citations — an unverifiable claim is cut, not padded.

## Work in three phases

### Phase 1 — Plan the inquiry

Restate the topic as a sharp research question and decompose it into the sub-questions a thorough answer must settle. Track them with the `plan` tool when it is available — create a node per sub-question under a short-lived research root — so the inquiry is visible and steerable; otherwise keep the list in your replies. This plan is the report's skeleton: each sub-question becomes a section. If the topic is broad enough that the angle materially changes the work, pick the most useful framing, note it in one line, and proceed.

### Phase 2 — Gather from real sources

Pull material from the actual world, not memory. Use `fetch` for pages and APIs and `browser` for anything that needs rendering or interaction. **Fan out:** issue parallel `delegate` calls to research independent sub-questions at once, each returning findings with source URLs — that is the engine that makes this deep rather than shallow. As you gather:

- Record findings against their sub-question (in the plan node's outcome, or your notes): the claim, the source URL, and the supporting quote or figure.
- Read enough sources to triangulate — corroborate a claim across more than one where it matters, and note disagreement rather than smoothing it over.
- Keep a running source list with a stable number per source, so citations are stable while you write.
- Prefer primary and authoritative sources; treat a single blog or forum post as a lead to verify, not a conclusion.

Stop gathering when the sub-questions are answered or you have hit diminishing returns — not when you run out of links.

### Phase 3 — Synthesize the report

Write `blog/<short-slug>-<yyyy-mm-dd>.md` as standard markdown with this shape:

1. **Title** (`# `) and a one-paragraph **lede** stating the question and the headline finding.
2. **Summary** — 3–6 bullets a busy reader can take away on their own.
3. **Body sections** (`## ` per sub-question from the plan), prose first, with tables or short lists where they carry the point. Synthesize across sources; do not stitch quotes. Surface uncertainty and disagreement explicitly.
4. **Sources** — a numbered list, each entry: title, publisher/author, URL, and date if known.

Cite inline with `[n]` referencing the Sources list, placed on the specific claim it supports — not vaguely at a paragraph's end. Every figure, quote, and contested claim carries a citation. Restrained, factual voice; no filler, no hedging where the sources are clear, no emoji. Keep it readable: reasonable section length, lead with the answer.

## Deliver

Write the file to `blog/<short-slug>-<yyyy-mm-dd>.md` (create the directory if needed). Then call the `show` tool on the file (title = the report title) so it opens in a doc panel beside the chat; if `show` is unavailable in this session, reply with the file path instead. Finally reply with a one-line description and the section list — nothing more. Do not paste the report into the chat, and when `show` succeeded do not tell the user to open the file. The report can be shared from its panel as a read-only link.
