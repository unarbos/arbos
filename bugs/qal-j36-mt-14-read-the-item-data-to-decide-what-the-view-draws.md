# qal-j36 — mt-14 read the item data to decide what the view draws

- **status**: fixed in the rig (this loop's defect, not a product defect), and replaced with a check worth having
- **found**: 2026-09-18 12:16, finishing the triage of cycle 8's desktop-step breaks
- **kernel**: `arbos-kernel 0.2.0 d2a807e48423 protocol 1`; app at `2301abd291c0`
- **rule it printed**: `mt-14-raw-done-card`
- **rollouts**: `20260918T121452Z-mt-14-…` (the standing break, with the matched item recorded), `20260918T121737Z-mt-14-…` (pass)

## What it claimed

`mt-14-child-done-does-not-interrupt` broke in **every cycle from 2026-09-17 18:18** — five in a
row — with:

```
mt-14-raw-done-card: a raw 'Turn ended. Last words: …' line is shown for a finished child
```

The scenario recorded only a count, `raw_done_cards: 1`, so nobody could see what had matched.
Recording the item itself gave:

```
{'kind': 'from',
 'text': 'Turn ended. Last words: I have successfully run the command `sleep 20; echo late`.…
          \n(transcript: .arbos/agents/late/transcript.jsonl)'}
```

Machine phrasing and an internal path, apparently drawn in a person's chat.

## Why it was not that

The raw text **belongs** in the item: it is the kernel's done file
(`crates/arbos-kernel/src/remote.rs:1327`), and the item is the transcript record. The *view*
cleans it. `done_report` (`desktop/src/view/component/transcript.rs:1455`) strips one of three
prefixes, truncates everything from `(transcript:`, and `worker_card` draws what is left as "one
dim line in the flow — no box, no header".

So the assertion searched the **data** and drew a conclusion about the **view**. The claim in its
own message — "is shown" — was never measured. The app has been rendering this correctly the whole
time.

## The check that replaced it

The regression this scenario was actually afraid of is real and is checkable from here: the
**wording coupling**. `done_report` recognises exactly

- `Turn ended. Last words:`
- `Turn ended badly. Last words:`
- `Turn stopped by the user.`

If the kernel's phrasing drifts, `done_report` returns `None`, the view has nothing to strip, and
the person does see the raw line — prefix, ellipsis and `.arbos/agents/…/transcript.jsonl` and
all. So `mt-14` now asserts that a finished worker's line begins with a prefix the view knows, and
names both sides in its `where`. It passes today:

```
done_report_prefixes: ['Turn ended. Last words: The command output: la']
composer_text: 'half a thought, not sent'
```

The composer half — a child finishing must not clobber unsent text — was passing all along and
still does.

## The lesson

**Assert on the layer your claim is about.** "A raw line is shown" is a claim about the view; the
item list is the data behind it, and a transformation sits between them. When the layer you can see
is not the layer you mean, either reach the right layer or assert the property that protects it —
here, the contract between the two sides of the transformation, which is stronger than the original
check because it fails *before* a person sees anything wrong.

## The pass this belongs to

Cycle 8's desktop step left four reds unexplained. Examined in order they were `qal-j33` (`xp-01`
typed at a window that was not listening and called the words lost), `qal-j34` (`mt-18` read a
height key that does not exist and called the column empty), this one, and `qal-j35` — the only one
that survived, a real fault where a relaunch comes back on the main chat instead of the sub-chat the
person left open.

Three of four reds were ours. That ratio is the argument for triaging a break before believing it,
and it is also the reason the fourth is worth believing.
