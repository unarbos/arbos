---
name: Weekly Planner
slug: playbook-weekly-planner
description: "Playbook: a short look at the owner's coming week: the meetings and deadlines that shape it, clashes and invitations to sort, and what you will get ready ahead of time. Read-only."
tags: [playbook, weekly, week, plan, planner, calendar, deadlines]
when_to_use: The owner asks to plan their week, or a routine fires that names this playbook.
---

# Weekly Planner

Follow the `playbooks` skill to offer, adopt, change or stop this on a schedule. Run it once on
request without adopting anything.

**Needs:** a calendar connector. Optional: email, chat, documents, GitHub, an issue tracker.
**Default schedule:** `0 18 * * 0`, Sunday around six in the evening their time.
**Never:** accepts, declines or moves anything; lists routine meetings; promises prep it does
not track.

**Preferences:** `send_time`; `routine_meetings` (recurring meetings to leave out, learned).
**State:** `prep_promised`, what you said you would get ready and for which meeting.

## Each run

1. The meetings that shape the week (not `routine_meetings`), deadlines, clashes and
   unanswered invitations.
2. What you will prepare ahead of time, and for which meeting. On a scheduled run, write it to
   `prep_promised` and create a one-off routine to do it before that meeting. A promise with no
   routine behind it is a promise you will not keep.

## What it sends

One short message: what shapes the week, what to sort out, what you will get ready. A light
week is one line.

## Learns

Use feedback to update `routine_meetings`. Keep every promise in `prep_promised` and clear it
once delivered.
