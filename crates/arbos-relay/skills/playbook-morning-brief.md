---
name: Morning Brief
slug: playbook-morning-brief
description: "Playbook: a short brief of the owner's day before their first meeting: meetings, clashes, a line of prep for the ones that matter, and what they owe today. Read-only."
tags: [playbook, morning, brief, daily, calendar, meetings, agenda]
when_to_use: The owner asks for a brief of their day, or a routine fires that names this playbook.
---

# Morning Brief

Follow the `playbooks` skill to offer, adopt, change or stop this on a schedule. Run it once on
request without adopting anything.

**Needs:** a calendar connector. Optional: email, chat, documents, GitHub, an issue tracker.
**Default schedule:** `30 7 * * 1-5`, about half an hour before their usual first meeting.
**Never:** accepts, declines or sends anything; replays the whole calendar; sends a brief after
the morning is gone.

**Preferences:** `send_time`; `skip_meetings` (recurring meetings they never need prep for,
learned); `what_lands` (the parts they act on, skip or correct, learned).
**State:** `last_run`, when the last brief was completed.

## Each run

1. If their first meeting has already started, skip today and post nothing: a stale brief is
   worse than none.
2. Today's meetings in their time zone, with who is in them and links. Flag clashes,
   back-to-backs and invitations they have not answered.
3. One line of prep for each meeting that matters: what changed since the last contact, what is
   owed, the one thing worth raising. Skip `skip_meetings` and anything obvious.
4. What they owe today or is overdue, and anything waiting on them.

Read the thread or record behind every claim. Say when a source could not be checked; a source
you could not read is never "nothing new".

## What it sends

One short message: their actions first, then the meetings with their prep line. Drop empty
parts. A clear day is one line. End with one line inviting corrections.

## Learns

Use their feedback and what they did with the brief to update `what_lands` and `skip_meetings`.
Do not infer disinterest from silence. Record a completed brief in `last_run`.
