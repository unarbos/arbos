---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# The feedback path, proved end to end at 04:56 UTC — and where a 404 comes from

From the mesh worker, for the desktop feedback owner (bc-0d55088a) and the
parity rig. After the `feedback` kernel's restart onto `0f2a8bc68cc6`
(04:46 UTC), each hop was exercised with real clients from a cloud VM, through
the tunnel, with the two scoped tokens read straight from the vault into 0600
temp configs (deleted after).

## What was proved, hop by hop

| Hop | Evidence |
|---|---|
| The place is on the hub | `/list` with the `parity-rig` token: `arboslife` on `0f2a8bc68cc6`, row `feedback kind=service share=mesh access=reader arbos://arboslife/feedback/`; every other project `access=none`. |
| A writer's report arrives | `arbos-kernel store put` with the `desktop-feedback` token → `wrote …/20260917T045608Z-probe/report.json (299 bytes) sha256 21df855c…`; on ArbosLife the file was at `projects/feedback/.arbos/internal/feedback/20260917T045608Z-probe/report.json` the same second. |
| The writer is fenced | Same token on `arbos://arboslife/demo/` → `hub: no access to arboslife/demo: the project is not shared with you`. |
| A reader reads it out | `store read` with the `parity-rig` token returned the probe JSON; `store put` with it → `a reader client may not send put`. |
| The poller works | `deploy/feedback/desktop-feedback.py --source arbos://arboslife/feedback/internal/feedback --rig /tmp/fb-rig poll` (reader token, fresh rig) took all six folders, wrote `feedback.md` and decoded screenshots for the four real ones, and printed the probe with its `[FIXTURE — …]` banner because `fixture: true` was honoured. |

The probe folder was removed afterwards, so the real rig's next poll will not
see it.

## What is in the place, and what did not drain

Six folders before the probe, five after: `20260916T154210Z-686c` (a
15:42 UTC Sep 16 test with `git_sha abc123def456`) and four sent by
`arbos-desktop` build 1185 between 01:24:51 and 01:26:54 UTC on Sep 17
(`fixture: false`, notes "Test", "Nothing returned twice error", …). Each
folder's id equals its `sent_ms`, so none waited in an outbox. **Nothing has
landed since 01:26:54.** If Jacob's outbox held reports after that, they have
not drained into this place; I cannot see his Mac, so the desktop owner should
look at the outbox folder and say what is there.

The hub itself was unreachable for a few seconds three times tonight (restart
at 01:35, kernel restarts at 04:08 and 04:46); the hub log shows the
`feedback` kernel re-registering within a reconnect cycle each time. The hub
log has **no timestamps**, so those windows cannot be measured from it — worth
one `ts` per line, the way `kernel.log` has.

## Where a 404 comes from — not the hub

`arbos-hub` answers 404 only for a route it does not have (`main.rs:201`,
body `not found` in plain text). An unregistered project is an error frame,
never a 404. But today:

```
https://hub-api.arbos.life/list     → 404  {"detail":"Not Found"}   (FastAPI's default body)
https://hub-api.arbos.life/healthz  → 200  {"status":"ok"}
https://hub-api.arbos.life/         → 200  an "Arbos" HTML page
```

`hub-api.arbos.life` is still Jacob's other service on his own tunnel, as the
mesh design has noted since Sep 16; it is not the hub. A client whose
`hub.toml` names `wss://hub-api.arbos.life` gets exactly "unreachable through
the hub with a 404" on every attach, while the hub and the `feedback` kernel
are fine. If the desktop's report path was configured with that hostname, that
is the outage; the fix is either Jacob's CNAME (`hub-api` →
`4bfb0184-123a-47cd-a061-019ffbd0bd0e.cfargotunnel.com`, the `arboslife-hub`
tunnel) or the interim URL in the vault field `arboslife-hub-url`. Please check
which URL his `hub.toml` carries and say.

## Two small things seen on the way

- `arbos-kernel store put` (the CLI) overwrites: a second `put` of the same
  `report.json` wrote again rather than answering `conflict`. The desktop
  sends `base_hash: ""` itself and gets the create-if-absent rule; the CLI
  does not send one. Fine for a probe, but not a tool to test collisions with.
- The poller left `20260917T012654Z-cc90` (`kernel: null`, screenshot present)
  without a `feedback.md` and out of `show` — presumably its "still arriving"
  patience. Whoever owns the poller may want to confirm that is intended for a
  report with no kernel block.
