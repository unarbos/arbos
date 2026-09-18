# qal-j33 — five cycles of `first-line-lost` were one click on a window not yet taking input

- **status**: fixed in the rig (this loop's defect, not a product defect)
- **found**: 2026-09-18 11:53, triaging cycle 8's desktop-step breaks
- **kernel**: `arbos-kernel 0.2.0 d2a807e48423 protocol 1`; app at `2301abd291c0`
- **rule it printed**: `xp-01-first-line-lost` — a **data-loss** rule
- **rollouts**: `20260918T115358Z-xp-01-…` (diagnosis), `20260918T115830Z-xp-01-…` (pass after the fix)

## What it claimed

`xp-01-dead-kernel-line-stays-in-its-project` broke in **every cycle from 2026-09-17 18:18 to
2026-09-18 11:45** — five in a row — with:

```
xp-01-first-line-lost: A's first line (sent while A had no kernel yet) is not on
A's transcript: A=['A-after-death-…'] home=[] B=['B-only-…']
```

A line a person typed, on no transcript and in no store. That is the most serious category the
loop has, and it was wrong.

## What was actually happening

`TwoProjects.send` did what every send helper in the library does:

```python
self.app.wait_element("composer-field", reachable=True)
self.app.click("composer-field")
self.app.type(text + "\n")
```

It never checked that the click landed. The app's own state carries `composer.focused` and
`composer.text` (`desktop/src/driver.rs:1273`), so I recorded both. The answer:

```
first_line_send: {'focused_after_click': False, 'composer_left_holding': '', 'accepted': False}
```

**The composer was never focused, and the composer was also empty** — so the keystrokes went
nowhere at all. Not held, not refused, not lost by the app: never delivered to it.

Re-measured with a two-second bounded poll rather than an instant read, in case I was timing the
round trip instead of the app: still false. So it is not a measurement artefact.

The telling detail is that the *second* send in the same scenario — the after-death line, through
the same code — always landed. Only the **first** send after launch fails. `reachable` says the
element exists; it does not say the window is taking input yet.

## The fix

One shared helper, `desktop_scenarios.focus_composer(app)`, that clicks until the app says the
field is focused, bounded at 15 s, and reports how many clicks it took:

```
first_line_send: {'focused_after_click': True, 'clicks_to_focus': 2, 'accepted': True}
first_line_landed_after_s: 15.0
a_user_lines: ['A-first-XP10056: …', 'A-after-death-XP10056: …']
```

`xp-01` passes. **Two clicks** — the first never lands, the second does.

Applied at every site that typed after a single click: `TwoProjects.send`
(`crossproject_scenarios.py`), `Desktop.send` (`desktop_scenarios.py`), and the journey rig's
`send` (`journey_scenarios.py`). The remaining instance is an inline click-then-type at
`landing_scenarios.py:511`, inside a scenario that passes; it is latent there for the same reason
the others were — its send is not the first after launch.

## Three failures had been wearing one name

`xp-01` now separates them, because the reading differs completely:

| what happened | rule | who is at fault |
|---|---|---|
| the click never focused the field | `probe-composer-never-focused` | this rig; the run proves nothing |
| the line sits in the composer after Enter | `xp-01-first-line-stuck-in-the-composer` | the product, but the person can see it |
| focused, emptied on Enter, on no transcript | `xp-01-first-line-lost` | the product, and it is silent loss |

## Where this belongs

The fourth rig fault today to wear a product bug's name — after `qal-j23` (driver off the store
mount), `qal-j24` (`new_chat` on a sidebar the app removed), `qal-j27` (three scenarios reading
`root` for a sub-chat) and `qal-j29` (`af-03` crashing when its assertion passed). This one wore
the worst name available for five cycles.

The general rule, which is review rule 8's sibling: **a driver that types must prove the app took
the keystrokes.** Clicking and typing is an instruction, not an observation. The app states
whether the field is focused and what it holds; a helper that ignores both can only report that
the words are not where it looked, which is true of a typo, a stale selector and real data loss
alike.
