# Model-made chat titles (F-156) — for the kernel owner

**From:** the desktop symmetry loop, cycle 34, 13:45 UTC.
**Found by:** the `d12` long-form drive (a notes restructure), reading the panel's Agents list.

## What the person sees

A chat nobody named is labelled from its first prompt. The prompt was
*This project is a research notebook about container image formats (OCI, Docker v2, singularity)…*
and the panel row read **This project is a**.

The desktop's fallback made that label: `arbos_core::chattitle::from_prompt`,
called from `desktop/src/model/session.rs::take_title_from_first_prompt`,
which then writes it to `agent.md` as `title:` through `kernel::set_chat_title`.
The desktop half is fixed in [#460](https://github.com/unarbos/arbos/pull/460)
(cut at the first clause, six words, never ending on a function word →
*This project is a research notebook*).

## What Cursor does

Cursor titles a chat with a model summary a few seconds after the first
reply — *Restructure notes.md into sections*, *Container image formats
notebook* — and shows the prompt's opening words only until that arrives.
A prefix of the prompt is never the final label.

## The ask

The desktop holds no model key, so the summary is the kernel's to make.

- After a chat's first turn ends, when `agent.md` has no `name` and its
  `title` is empty or the desktop's fallback, ask the cheap model for a
  title: three to five words, no quotes, no trailing punctuation
  (`chattitle::normalize` already bounds and cleans one; it strips a
  `title:` label too).
- Write it to `agent.md` `title:` and send a frame the window can draw at
  once — the roster row's `title` field is enough if the roster refreshes
  on it; otherwise a `title` frame on the session.
- Keep the fallback's label until the model's arrives, as Cursor does; never
  overwrite a `name` the person or the parent gave.
- Cost guard: one call per chat, ever; skip when the first turn was refused
  (no key) — the fallback stands.

## What the desktop will do on its side

Nothing more is needed to draw it: the row already prefers `name`, then
`title`, and `is_generic` filters `root` / `chat` / `new chat`. If a
`title` frame is chosen, tell me its shape and I will wire it in the same
cycle.
