---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Live hub rebuilt from `dd7814fc` (#545): sleeping machines stay on the roster

Mesh worker, 2026-09-18 01:47 UTC.

**Deployed:** the ArbosLife hub now runs `main` `dd7814fc`, which carries
[#545](https://github.com/unarbos/arbos/pull/545) (and #538/#540 from the
previous rebuild). Built here (`cargo test -p arbos-hub`: 14 passed), file
swapped first, `systemctl --user restart arbos-hub.service`; all seven
ArbosLife projects and the worker re-registered within 8 s.
`GET /list` rows now carry `online: true|false`, `offline_since_ms` when
offline, and a machine whose last registrant left keeps its row with its
projects as sleeping (`live: false`) for up to 7 days (`OFFLINE_KEEP_MS`,
in memory; a hub restart forgets sooner).

**Proved on the live hub, not assumed.** A scratch kernel registered as
machine `podtest` through the public tunnel, then its only kernel was stopped:

```
01:43:31 podtest online=True  offline_since=None      projects: [(place, live=True)]  builds: 1
01:43:36 podtest online=False offline_since=01:43:31  projects: [(place, live=False)] builds: 0
01:43:56 podtest online=False offline_since=01:43:31  projects: [(place, live=False)] builds: 0
```

The row stayed, went `online: false` within five seconds, kept its project as
a sleeping row, and stamped `offline_since_ms` with the moment the last
registrant left. That is what a phone needs to show "ArbosLife is asleep"
rather than an empty list.

**Cleanup:** the test row would have sat on Jacob's phone as a sleeping
machine `podtest` for a week, so the hub was restarted once more at 01:46
(a five-second blip, everyone re-registered); the roster is `arboslife` only.
The scratch kernel and its config are removed from this VM.

**Not done, by instruction:** `v0.2.0` was not published. **Nothing else
moved**: ArbosLife's kernels are unchanged (`last_activity_ms` from #538 still
appears only once they are on a build at or past `11a01d84`).
