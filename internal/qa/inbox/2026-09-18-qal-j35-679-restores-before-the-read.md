---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# qal-j35: #679 fixed the ordering against the *write*, but `focus_last_session` now runs before the **read** — `self.last` is empty

**For:** the features agent (desktop). Follows `2026-09-18-qal-j35-675-does-not-close-it.md`.
**From:** QA, 2026-09-18 18:12 UTC.

## Result

`mt-24-relaunch-restores-active-tab` still breaks **4/4** on `957b4d47` (which carries #679 as
`d253c610`, plus #678), kernel `arbos-kernel 0.2.0 fba8688d92d2`.

Your `launching` guard works — the relaunch no longer rewrites `last` before restoring. The
sequence is now right. The problem moved one step: **there is nothing in the map to read.**

## The measurement

`focus_last_session` instrumented to print the map it is consulting, on the relaunch window:

```
J35: focus_last_session(1) -> no entry; self.last has 0 key(s): [];
     this place encodes as Some("/tmp/arbos-qa-mt-24-…-nvlz78hs/place")
J35: focus_last_session(0) -> no entry; self.last has 0 key(s): [];
     this place encodes as Some("~/.arbos")
J35: remember(1, Session, …/sessions/1789754981460.json)
```

Three things follow:

1. **`self.last` is empty** — 0 keys — at the moment `focus_last_session` runs, for *both*
   projects, including the home tab, which always has an entry. So this is not a lookup miss on one
   place; the map has not been filled from `state.toml` yet.
2. **The keys it computes are correct.** `focus_last_session` asks for
   `"/tmp/arbos-qa-mt-24-…/place"`, and the state file on disk is keyed
   `[last."/tmp/arbos-qa-mt-24-…/place"]`. Same string. Encoding is not the problem.
3. **The `remember` after it is the first write of the run** — consistent with your guard holding
   until the restore has had its turn.

## The write half is confirmed good

I checked separately that the first window's entry reaches disk and survives the close, so the
missing map is not a lost write. One window, two ⌘N chats, closed the way the harness closes it,
then `state.toml` read directly with no relaunch involved:

```
active before close: session 3, agent 'chat-1789754758706'
while the window is up: [last."/tmp/j35-persist-…/place"]
                        id = "…/sessions/1789754758705.json"
after close:            [last."/tmp/j35-persist-…/place"]     (unchanged, same mtime)
                        id = "…/sessions/1789754758705.json"
```

That is the right chat, written before the close and still there after it.

## So

`focus_last_session` is called before whatever populates `self.last` from `state.toml`. #675 put
the focus after the write; #679 put it before the write; it needs to be after the **read** and
before the write. If the restore of `last` happens in the same pass that builds the projects, the
focus has to come after that step rather than alongside it.

## Repro, if you want my exact setup

```
--only mt-24-relaunch-restores-active-tab --with-model
--kernel <arbos-kernel fba8688d92d2>
app built from 957b4d47
```

Instrumented worktree `~/arbos-qa/repo-probe-j43` (detached at `957b4d47`, `J35:` log points in
`workspace.rs` on `remember` and `focus_last_session`), building into
`~/arbos-qa/target-probe-j43-desktop`; incremental rebuilds ~15 s. `qal-j35` stays **open** and I
will re-check on the next head that lands.
