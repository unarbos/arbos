# ADR-0037 — Discussion branching: anchored sub-discussions that merge back

- Status: Accepted
- Date: 2026-06-19
- Refines: ADR-0027 (control-seam fork/switch), ADR-0015 (injected context)

## Context

A reader of a long conversation often wants to interrogate one specific claim —
a sentence the agent wrote, a fragment of a tool result — without derailing the
main thread or losing the conclusion afterwards. Fork (ADR-0027) is the wrong
tool: it copies a whole prefix into an independent session that then drifts away
forever, and it rebinds the connection so the parent is abandoned. What we want
is the opposite shape: open a *small* side discussion scoped to a highlighted
span, reach a conclusion in it, and fold that conclusion back into the parent so
the main thread is the richer for it.

The parent conversation is an append-only event log projected into the model
context (ADR-0008, ADR-0015). Two constraints fall out of that. First, the
highlighted span addresses a *frozen* event — its `Seq` and text will never
change, but later compaction (ADR-0014) can fold the surrounding range, so the
anchor cannot rely on the event still being individually present. Second, the
merge-back must reach the model the same way every other piece of derived
context does — as injected, fenced, supersedable context — not as a forged user
or assistant turn, or it corrupts the trajectory as training data and breaks
prefix caching.

This is a new persisted event kind and a cross-session data shape: costly to
reverse, so it gets an ADR (ADR-0001).

## Decision

**A discussion branch is a child session anchored to a highlighted fragment of
a parent event, resolved by accepting (merge a curated summary back) or
discarding it.** No new identity or storage mechanism: it composes the existing
session tree, the event log, and the latest-per-source context projection.

### The anchor is a logged event with no conversational projection

A new sealed payload `core.BranchAnchorPayload` (kind `branch_anchor`,
`CurrentEventVersion` unchanged — purely additive, ADR-0010) records the branch:

- `Branch` — the child `SessionID` this anchor opened.
- `Seq`, `Start`, `End` — the parent event and the rune offsets of the
  highlight into its *rendered* text (offsets are best-effort).
- `Quote` — the highlighted text, **denormalized** so display survives the
  parent event being compacted away. The append-only log freezes the referenced
  event, so the quote is the display source of truth, not a live read.
- `Message` — the full text of the *containing* message. This is the branch's
  entire context: the child discusses the fragment with its own message around
  it, not the whole thread (see scoping below).
- `Status` — `open` | `accepted` | `discarded`.
- `Summary` — the curated conclusion captured on accept (empty otherwise).

`ProjectEvent` returns `(Message{}, false)` for `branch_anchor`: the anchor is
bookkeeping the model must never see. Excluding it at the single event→Message
mapping home keeps it out of every projection at once — live turn, replay,
compaction folding, and recall. The `exhaustive` linter forces this decision to
be made for the new kind rather than silently defaulted.

### The branch is scoped to the fragment, not the thread

`sessiontree.Branch` opens the child with **only** the anchor's `Message` as
seed context (a `ContextPayload` segment, source `branch_anchor`), with the
highlighted `Quote` called out within it. A branch opened mid-thread therefore
shows the model the one message the highlight came from — no later turns, no
full history. This is the deliberate distinction from fork: fork copies a
prefix; a branch carries a fragment. It writes the open anchor on the parent and
a self-anchor on the child (`Branch == newID`) so the child knows what it is.

### The latest anchor per branch is authoritative

A branch's lifecycle (`open → accepted` / `open → discarded`) is recorded by
appending successive `BranchAnchorPayload`s, never by mutating the log.
`core.LatestBranchAnchor` reads the current status by scanning back for the most
recent payload for that branch — mirroring the latest-per-source (ADR-0015) and
latest-config supersession rules. Re-accepting a branch is just another append.

### Accept merges back as a fenced, supersedable context segment

`sessiontree.Accept` appends, to the **parent**, a `ContextPayload` segment
under source `core.BranchSegmentSource(branch)` = `"branch:" + childID`, then an
`accepted` anchor carrying the summary. Because the source is unique per branch,
distinct branches *accumulate* (each its own fenced block
`<<branch:CHILD>> … <</branch:CHILD>>`) while re-accepting the *same* branch
*supersedes* its prior summary. The conclusion thus enters the model exactly
like memory, the job table, or the plan forest — injected, fenced, lean, and
prefix-cache-stable — and survives the parent's own compaction. `Discard`
appends a `discarded` anchor and merges nothing; the child transcript is left
intact for the record either way.

### Driven from the control seam, never as a turn intent

These are control-plane operations — they manipulate the store and create
sessions, they do not advance a turn — so they live at the control seam beside
fork/switch (ADR-0027), not as engine `Intent`s. `control.Serve` gains three
client frames: `branch` (`{new_session_id, anchor_seq, anchor_start, anchor_end,
anchor_quote, anchor_message}` → `branched`), `accept_branch`
(`{new_session_id, summary}`), and `discard_branch` (`{new_session_id}`). Unlike
fork, `branch` does **not** rebind the connection: the parent stays open so the
side discussion is genuinely a side discussion. The engine exposes
`BranchSession` / `AcceptBranch` / `DiscardBranch`, each a thin wrapper over
`internal/sessiontree` so the engine and agent layers share one implementation
with no import cycle (the ADR-0027 arrangement).

## Consequences

- One new event kind (`branch_anchor`), additive: no schema bump, no upcast
  rule, old logs decode unchanged (ADR-0010). The codec gains one case.
- The merge-back reuses the ADR-0015 context machinery wholesale, so accepted
  conclusions inherit fencing, latest-per-source supersession, and compaction
  survival for free — the reason the feature needed no new persistence concept.
- A branch's conclusion is curated text the human approves on accept, not the
  raw child transcript: the parent context stays lean and the human controls
  what crosses the boundary.
- The denormalized `Quote`/`Message` on the anchor mean a branch keeps
  rendering correctly after the parent event it points at is compacted away —
  the cost is that the highlight cannot be re-derived if the rendering of the
  source event later changes; the frozen copy wins, which is correct for an
  append-only log.
- `branch` deliberately does not rebind, which makes "open several branches off
  one parent" natural and keeps the parent the home thread.

## Alternatives rejected

- *Reuse fork for this* — fork copies a whole prefix and rebinds the connection,
  abandoning the parent and never merging back. A branch is a fragment-scoped
  side thread that returns its conclusion; the shapes are opposites.
- *Merge the conclusion back as a synthetic user or assistant message* — forges
  the trajectory (corrupts it as training data, ADR-0011) and is not
  supersedable, so re-accepting would duplicate rather than replace. Injected
  context is the honest channel.
- *Anchor by live reference to the parent event* — breaks the moment compaction
  folds that event. Denormalizing the quote and containing message onto the
  anchor is what makes the highlight durable on an append-only log.
- *A mutable status field updated in place* — violates the append-only log.
  Latest-anchor-per-branch gives the same answer by supersession, consistent
  with every other "latest wins" rule in the kernel.
- *Model these as engine Intents* — they create sessions and touch the store
  across the tree; that is control-plane work, placed at the seam beside fork
  per ADR-0027.
