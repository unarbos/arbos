---
name: Watchlist
slug: playbook-watchlist
description: "Playbook: keep an eye on a repository, a project, a page, a channel or a question and say only what changed or needs the owner. Read-only."
tags: [playbook, watch, watchlist, monitor, track, changes, alert]
when_to_use: The owner asks you to watch or keep an eye on something for them, or a routine fires that names this playbook.
---

# Watchlist

Follow the `playbooks` skill to offer, adopt, change or stop this on a schedule. Each watch is
its own adoption: a settings file `playbooks/watchlist-<slug>.md`, where `<slug>` names what it
watches, and its own routine. When adopting, ask only what you cannot infer: what to watch and
what is worth telling them.

**Needs:** access to whatever it watches.
**Default schedule:** `0 9,15 * * 1-5`, twice a weekday.
**Never:** posts, comments or reacts in what it watches; pads a quiet run; reuses an old number
instead of checking again.

**Preferences:** `question` (what it watches or answers, in their words); `sources` (the
repositories, projects, pages or searches it reads, with links); `needs_them` (what is worth
telling them: they were mentioned, a review or decision is waiting, something broke).
**State:** `last_pass`, when the last run looked, so the next one reads only what is new.

## Each run

Read `sources` since `last_pass`. Tell them only what matches `needs_them`, then set
`last_pass`. A source you could not read is a gap, not "nothing new": record it in state and do
not move `last_pass` past it, so the next run looks again.

## What it sends

A short message of what changed or needs them, each with a link. With nothing new, post
nothing.

## Learns

Use feedback and what they did with a report to refine `needs_them` and `sources`. Silence
alone teaches nothing.
