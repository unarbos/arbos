---
name: Playbooks
slug: playbooks
description: How a playbook (a skill whose slug starts with "playbook-") is offered, adopted on a schedule, changed, stopped and run with the owner's own settings.
tags: [playbook, routine, schedule, recurring, proactive, offer]
when_to_use: The owner asks what you can do regularly, names a cadence ("every Friday", "each morning"), asks for the same thing at the same time twice, wants to set up, change or stop a recurring job, or a routine fires that names a playbook.
---

# Playbooks

A playbook is a built-in skill for a repeatable job. Its slug starts with `playbook-`;
`skills_list` with `q: "playbook"` shows them. Running one once on request needs nothing
else. Adopting one gives the owner a routine and a small settings file; the skill itself stays
shared, so an improvement to it reaches every adoption on its next run.

## Settings file

One per adoption, in the project folder at `playbooks/<name>.md`. `<name>` is the playbook
(`morning-brief`) or, for one adopted more than once, the playbook and a slug
(`watchlist-release-prs`). It holds only what is personal:

```yaml
---
playbook: morning-brief
preferences:
  skip_meetings: [Daily standup]
  format: Five bullets max
state:
  last_run: "2026-11-23T15:30:00Z"
---
```

`preferences` holds the owner's inputs and their differences from the skill's defaults; every
playbook also accepts `format`. `state` is working data between runs. Never copy the skill's
steps, defaults or rules into it. Preferences override defaults, never the skill's **Never**
list and never an approval rule. Record what the owner said directly; label what you inferred
with the evidence for it. Silence is not evidence of dislike.

The routine is the only record of timing. Find it with `routines_list`: its name is
`playbook:<name>`. Do not copy its id, cron or time zone into the file.

Keep one dated line per playbook in `playbooks/offers.md`: offered, declined, adopted or
stopped. It is the history of what was offered, not a copy of the settings.

## Adopt

Only on the owner's yes:

1. Check the playbook's **Needs** with `connectors_list`. If something required is not
   connected, say which and stop.
2. Write the settings file with what you already know and empty state.
3. Check `routines_list` so the same `<name>` is never scheduled twice. Then `routines_create`
   with the name `playbook:<name>`, a cron trigger (the skill's default unless they said
   otherwise) in their time zone, and the prompt "Run the <playbook> playbook with the
   settings in playbooks/<name>.md." Ask their time zone only if no message has carried it.
4. Record it as adopted in `playbooks/offers.md` and tell them when it runs. Run it once now
   if that is still useful.

If scheduling fails, keep the settings file and say it is not scheduled.

## Change, stop, run

- **Change.** Edit preferences in the settings file. To change timing, `routines_update` the
  `playbook:<name>` routine; if that fails, say so.
- **Stop.** `routines_delete` the routine and record it as stopped. Keep the settings file so
  it can be resumed.
- **Run.** Read the playbook's skill and the settings file, apply the preferences and do the
  work inside the approvals you already have. A scheduled run is your own instruction, not a
  yes from the owner: anything that sends, posts, books, buys, deletes or changes something
  outside the project still asks first. Update only the state and preferences that changed.
  If the skill is gone or a preference no longer fits it, say what needs fixing instead of
  guessing.

## Offering

Offer a playbook when the owner asks what you can do, or when they have as good as asked: they
name a cadence, or make the same request at about the same time twice. Read the playbook's
skill before you offer it, and check `playbooks/offers.md` first.

- One or two sentences: the concrete thing you would do, why it helps now, and a plain
  invitation to say yes.
- At most one unasked offer in any 24 hours, across all playbooks and every other suggestion.
  That is a ceiling, not a target; most days have none.
- Never in the middle of a task, never in a chat with other people in it, never during a
  scheduled run of something else.
- An offer that got no answer is not repeated or repackaged, and no other unasked offer
  follows it for seven days. A declined one stays declined until the owner reopens it.
- Record the offer in `playbooks/offers.md` before you send it. If you cannot read or write
  that file, stay quiet.
- An offer is not an adoption. Wait for the yes.

## A job no playbook fits

Offer an ordinary routine for it. If it would be useful to others, save it with `skills_save`
in the shape of the built-in ones, without the owner's private names, sources or accounts.
