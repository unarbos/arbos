---
cursor:
  subagentId: "bc-fddc4e9f-b8ac-5d93-a0fa-0cb3ec84a180"
---

# Terminal and Browser open speed (PR #692)

Status: [#692](https://github.com/unarbos/arbos/pull/692) is open, out of draft, mergeable.
CI green on `5b65c1cc`, all six checks. Branch
`cursor/faster-terminal-and-browser-open-a180` off `4db6f8ee`.

## The two causes

**Browser — the click killed the kernel.** `Frame::Browse` ran the whole of
`BrowserHub::act("navigate")` inline on the kernel's one frame loop: spawn
Chromium, poll for its devtools port, open a WebSocket, wait for the page
load, read the title, run the model-facing snapshot script. Two consequences,
and the second is the one a person felt:

1. every other client frame in the place queued behind it;
2. `reqwest::blocking` drops a Tokio runtime, and dropping one on a runtime
   thread panics — `Cannot drop a runtime in a context where blocking is not
   allowed`. The kernel exited. Reproduced directly: a `browse` frame sent to
   a hand-started kernel, then EOF on the socket 300 ms later, with the panic
   in the kernel's stdout log.

The browse is on a blocking task now. Two smaller things on the same path:
`ensure_chrome` serialises its start (a `browse` and the screencast it turns
on arrive a frame apart, and the second launch deleted the
`DevToolsActivePort` the first had just written, so both waited out the 25 s
deadline), and the window's open takes navigate plus one picture on one CDP
connection rather than also reading the title and the page snapshot.

**Terminal — the pane's own socket could never hear the prompt.** The pane
called `attach_or_spawn_place` on every open (canonicalize, bootstrap
`.arbos/`, probe the port, `GET /healthz`, compare kernel commits) and then
opened a second attach, which the kernel answers with a greeting, a snapshot,
a plan frame per agent and the focused chat's transcript tail. By then the
prompt had already gone out over the connection the window was holding, and
an empty `pty_in` makes no shell print again. The pane showed "Connecting…",
then a black rectangle, until the person typed.

The pane now reads the window's connection. Output is kept per page on the
project (`desktop/src/model/pty.rs`) and handed over when the pane mounts,
prompt included. Keys go back the same way.

## Numbers

Same two harnesses run at `4db6f8ee` and on the branch, same machine, real
kernels. `crates/arbos-kernel/tests/open_speed_e2e.rs` and
`desktop/tests/open_speed.rs`; `--nocapture`, lines start with `MEASURE`.

| measure | before | after |
| --- | --- | --- |
| `browse` → `board` | 41 ms | 47 ms |
| `shell` asked one frame behind a `browse` | never (kernel gone) | 47 ms |
| `browse` → first picture | never (kernel gone) | 0.3–1.2 s (Chromium's own start) |
| `shell` → `board` | 44 ms | 43 ms |
| `shell` → prompt on the window's connection | 207 ms | 200 ms |
| `shell` → prompt on a socket of the pane's own | none in 5 s | none in 5 s |
| `attach_or_spawn_place` preflight, warm | 6 ms | 8 ms |

The last three rows are the same before and after on purpose: both routes
behave identically on both commits, and what changed is which one the pane is
on. The preflight is cheap on an empty scratch place on Linux; on a real
project it is a canonicalize, a `.arbos/` bootstrap with a git exclude, a TCP
probe and an HTTP round trip, and it ran on every terminal open.

## The red CI run, and what it was

The first run failed one check: `port_health_e2e`, which times a kernel's
startup, with a broken pipe on its first write. Not a pre-existing flake —
it was the only failure in the last sixty runs of the repo.

It was my harness. The two browse measures started two kernels, so two
Chromiums, and a test ending in `child.kill()` sends SIGKILL: the kernel
never returns from `run`, never drops its `BrowserHub`, and the browser it
started outlives it. Measured on this box: **seventeen Chromium processes and
two `/tmp/arbos-chrome-*` directories left behind by one run of the file**,
resident for the remaining twenty minutes of the suite on a four-core runner.

`5b65c1cc` answers all three browser questions from one kernel and one
Chromium, and stops its kernels with a signal they handle (SIGINT, which the
serve loop breaks out of cleanly so `BrowserHub::drop` runs). After that: no
chrome process and no profile directory left. CI green.

**Worth a follow-up, not taken here:** `browser_e2e` and
`browser_takeover_e2e` end the same way and leak the same way. Widening this
PR to them did not seem right, but the suite pays for it on every run.

## What is not verified

I could not run the Mac app, so the terminal change rests on the desktop test
suite (101 lib tests plus the new integration harness, green) and on reading.
The riskiest hunk is `Arbos::sync_terminal` and the new `feed_terminal` in
`desktop/src/view/root.rs` — a click on the Mac would be worth having before
this is trusted in a release.

Chromium's cold start is still there on the first Browser open of a kernel's
life. Pre-warming it when the drawer's tab cards are shown would hide it, at
the cost of launching a browser because somebody opened a tab picker.

## Scope held

v0.2.0 not published. Jev untouched. The chrome clutter list untouched
(another worker owns it). qal-j35 untouched.
