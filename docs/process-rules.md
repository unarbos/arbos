# Process rules — what reaches `main` and who decides

Decided by Jacob; kept in the repo so the loop, its agents and every worker read the same text.

## Outside feedback never merges without Jacob's per-item approval (2026-10-24 13:55 UTC, permanent)

- **Who is "outside":** every reporter that is not Jacob's own account (`unarbos`, `arbos`, `const`) and not
  a coordinator-worker-owned item. Testers (TestFlight, in-app sheet), Doug, agent-filed reports on
  behalf of other users, group members, relayed chat reports — all outside.
- **What still happens automatically:** intake → malicious-intent screen → fix agent → tests → CI gate
  (Linux + Mac release build + iOS build on the integration tree). The work is done and verified.
- **What does not happen:** the merge. A green outside fix holds as **`awaiting-jacob`** (loop state
  `awaiting_jacob`, GitHub label `awaiting-jacob`), its branch is kept current with `main` (main merged
  in whenever main moves; re-gated when main touched its files), and it is listed in
  `/var/lib/arbos-feedback/awaiting-jacob.md` on the relay host (mirrored to the project store's
  `docs/awaiting-jacob.md`), one row per item: issue, reporter, what/why, platform, before/after links,
  ready-since, state.
- **Decisions are per item, by Jacob:** `sudo arbos-feedback approve <item>` merges and ships it through
  the normal release; `sudo arbos-feedback decline <item> "<note>"` closes the issue with the note and
  keeps the branch. Nothing else moves an awaiting item.
- **No wholesale lift.** There is no "turn the gate off", no "restart the loop", no time-based release.
  The gate is on by default in the poller (`APPROVAL_DEFAULT_ALLOW`); `arbos-feedback approval-gate off`
  is refused. If an operator ever must lift it, `approval-gate lift --i-am-jacob --reason "<why>"` is
  required, is written to the loop log and shown in `status`, and `approval-gate restore` re-arms it —
  and even a lift does not release already-held items; they still need `approve`.
- **Jacob's own asks** (reporter `unarbos` / `arbos` / `const`, items labelled "Jacob (owner)", items
  owned by a coordinator worker, items the coordinator tracks) merge and ship as before.
- **Jacob's design rules are not overridden by tester preference.** `docs/mac-design-rules.md` (centred
  header pill, hover-only timestamps, the three-control sidebar strip, three panes, right-pane layout, …)
  stands even when a tester report asks for something else; a fix that would move one of those rules
  holds for his approval like any other outside change, and the agent says so in its summary.

## The loop merges the live branch head, never an older one (2026-10-25)

- A gate run gates one commit per branch. When the run passes, the loop merges that commit **only if it is
  still the branch's head**. If the author pushed after the run started, the gated head is not merged: the new
  head re-gates and merges on green, and the issue says so. Nothing is orphaned by a merge.
- A push to `feedback/<id>` **after** the item merged (the loop deletes the branch on merge; the next push
  recreates it) is followed automatically: the loop makes `feedback/<id>-followup` (…`-followup2`, …) from that
  head, registers `<id>-followup` on the same issue and owner, gates and ships it, and notes it on the issue.
  Authors do not re-land tails as new issues (#1684, #1721 were that).
- **A follow-up inherits its source item's reporter and tier.** If the source's reporter is anyone other than
  Jacob (or a coordinator-owned item), the follow-up holds as `awaiting-jacob` and merges only on an explicit
  per-item `approve` — no timer, no owner-confirm shortcut. Silence is not a yes. Jacob/coordinator-source
  follow-ups get a one-hour owner-confirm hold, then gate as normal.
- Authors: one branch per item is fine, but once the item merged, push the tail to the same branch and let the
  loop pick it up — or open the follow-up item yourself if you want a different scope.
- The merge-revert probe (#1588) runs on the head that is about to be gated/merged; with this rule that is
  always the live head, so the two compose: a stale gated head is never merged, and a live head whose merge
  commit reverts main's files is sent back before integration.

## iOS unit tests on the shared EC2 Mac (2026-10-25 19:30 UTC)

The EC2 Mac is the only Mac the iOS workers have, so running the iPhone unit tests there is allowed — under
these rules, because the box also runs the Rust/iOS gate and the Mac release + TestFlight runner on 8 cores
and a 250 GB volume (five leftover 13 GB test-simulator clones took it from 60 to 14 GB free on 2026-10-25):

- `xcodebuild test` with `-destination` naming **one** existing simulator (`-destination 'platform=iOS
  Simulator,name=iPhone 17'` or `id=<udid>`), never a generic destination that makes Xcode clone a device.
- `-parallel-testing-enabled NO` — parallel testing is what creates the "Clone N of iPhone 17" devices
  under `~/Library/Developer/XCTestDevices`.
- Leave nothing behind: delete any clone your run made (`xcrun simctl --set ~/Library/Developer/XCTestDevices
  delete <udid>`) and shut down the simulators you booted (`xcrun simctl shutdown <udid>`). Do not erase or
  delete the shared named simulators (`Arbos *`, `iPhone 17`).
- Build in your own checkout and derived-data path (`-derivedDataPath ~/<your-id>-dd`); the gate's trees
  (`~/arbos-gate`, `~/arbos-gate-dd-*`, `~/arbos-rust-gate-target`) and the runner's are not yours.
- The clean script's sweep (idle clones > 30 min, idle rig checkouts > 30 min, stale `Deleting-*` dirs) is the
  backstop, not the plan; a run that leaves the box under 20 GB free is a regression to raise on your issue.

## One avatar resolver on both platforms (Jacob, 2026-10-26, ARB-394)

- Every agent avatar on the Mac and the iPhone — typing/status/activity row, chat header, sidebar
  and chat list, bubbles, mention chips, profile hero, members, search, notifications, call tiles,
  Secrets picker — is drawn through one resolver keyed by agent id + `avatar_version`: Mac
  `ui::avatars` (`project_in` / `project_with` / `member_picture` / `agent_identity` +
  `agent_picture`, newest copy from `sync::agent_avatar`), iPhone `AgentAvatar(project:)`
  (resolves through `AppModel.newestAgentAvatar`) and `model.projectImageURL(_:)`.
- No surface builds its own picture URL or reads the copy it was handed (`.image_url` /
  `.imageURL`). `scripts/avatar-lint.sh` scans both trees in CI's Layout job and fails the build;
  a line that must draw a group / server / person picture carries `// avatar-lint: allow (<why>)`.
- The stamp is the only truth: a newer known version never loses to a cached or older copy; at
  equal stamps the caller's order holds (chat roster, my records, other rosters). While a picture
  loads, the disc is the neutral initial — never an older picture, never the leaf.

## No Simulator.app on the gate Mac (2026-10-28 21:40 UTC)

Xcode 27 on the shared EC2 Mac ships **no Simulator.app** (nothing under `Contents/Developer/Applications`,
nothing elsewhere on the box). Rig steps that `open -a Simulator` and then tap with `cgclick` can never work
there — eight such runs from several owners failed with "no Simulator window" after holding a heavy slot. The
runner's PATH shim (`/Users/Shared/arbos-run/axbin/open`) now fails such a call fast (exit 66) with this note.

Working path for iPhone shots and flows:

- boot the named simulator you own (`xcrun simctl boot <udid>`), install the build (`simctl install`), and
  drive the UI from an **XCUITest** target (`xcodebuild test-without-building -xctestrun <…> -destination
  "id=<udid>"`) — taps, swipes and typing go through XCUITest, not the window;
- capture with `xcrun simctl io <udid> screenshot <file.png>` (or `recordVideo`), which needs no window;
- shut the simulator down when done (`simctl shutdown <udid>`), as the heavy-slot rules already require.

Do not install Simulator.app or any other system-wide tool on the shared Mac without the coordinator's go-ahead.

## A gate that cannot test does not pass the branch — and how to ship anyway (2026-10-30 23:50 UTC)

When the iOS gate's unit run never reaches a verdict — no simulator named `iPhone 17` on the rig, no
`.xctestrun`, a kill before the first case — **no test ran, so the branch was not checked.** Until
2026-10-30 `ios-gate.sh` printed "could not run — not blamed" and returned a pass for that, which let
branches merge as tested when nothing had run. "Not the branch's fault" and "the branch passed" are
different statements and only the first one is true.

It now exits 2, the gate's infrastructure code, the same as a failed fetch: `rust-gate.sh` reports
`infra`, the poller retries on the next tick, no verdict is recorded and no issue is told its branch
failed. A transient rig fault heals by itself.

A lasting one summons a person. The gate keeps retrying — red would blame branches for the rig — but
after **six infrastructure verdicts in a row**, about twelve minutes at the poller's two-minute tick,
the poller files **one** issue labelled `gate-rig`: how long the rig has been untestable, which items
are waiting, and the gate's own repair steps. One issue per untestable stretch, not one per tick, and
it closes itself as soon as the iOS half returns a real verdict. `arbos-feedback status` shows the
same thing as `iOS GATE RIG UNTESTABLE since …` from the first verdict, before the issue exists.

Two operator overrides, both one-line files in `~/arbos-gate` on the gate Mac (ssh via the relay
host, below). The gate's failure message names both, with the commands, so nobody has to read the
script at 3am:

| File | Effect | Lifetime |
|---|---|---|
| `.test-sim` | The simulator to run on, when the rig has no device under the default name. `echo 'iPhone 16' > ~/arbos-gate/.test-sim` | Stands until removed |
| `.allow-untested` | Ships **one** branch the rig could not test. `echo 'jacob: P0 relay fix must go tonight' > ~/arbos-gate/.allow-untested` | Consumed by the next gate run |

`.allow-untested` is deliberately one-shot and deliberately requires a reason: the reason is printed
into the gate output, which the poller posts on the issue, so a merge without a test run is on the
record with a name against it. It does not weaken anything else — the compile halves, the
test-target compile and the known-red baseline rules all still apply. `ARBOS_GATE_TEST_SIM` does what
`.test-sim` does for a gate you run by hand; the poller's ssh session carries no environment of
yours, which is why the pins are files.

## The per-case tally is the failure count, and a disagreement is itself a failure (2026-10-31 22:30 UTC)

**Never count `error:` lines to count test failures, and never read `Executed N tests` to decide
whether a suite ran.** The two test targets report in two formats and only one of them prints either.

| | reporter | a failure prints an `error:` line | an `Executed N tests` summary |
|---|---|---|---|
| `ArbosUITests` | serial | yes | yes |
| `ArbosTests` | parallel clones | **no** | **no** |

So a failing unit test cannot appear in a count of `error:` lines. Not "is easy to miss" — cannot
appear. Measured on hosted run
[41239073951](https://github.com/unarbos/arbos/actions/runs/41239073951) at head `4c2db3338`:
`Fastfile`'s `lines.grep(/error:/)` announced `xcodebuild test failed (3 error lines)` and named
three UI tests. **There were four failures.** The fourth,
`HomeListFilterTests/testTheWiderRowStillFitsEveryPhoneAtTheDefaultTextSize`, sat at line 11,989 of
12,405 with no `error:` line and no `Executed` summary anywhere near it. The `** TEST FAILED **`
banner was there and no failing-test list printed at all. That log holds 512 parallel-shape case
lines and 39 serial ones, and all nine of its `Executed N tests` summaries belong to the UI target.

A tally of per-case lines across **both** formats is the only reading that finds it:

```
Test case 'Suite.testName()' failed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.002 seconds)
Test Case '-[ArbosTests.Suite testName]' failed (0.002 seconds).
```

**When the two readings disagree, the tally wins and the disagreement is printed.** The lane names
every failing case, says that the console showed a different number of `error:` lines, and says why
they differ. It is not reconciled quietly: a gate that prefers the smaller number is how four
failures were announced as three. The rule runs in both directions — if xcodebuild exits 0 while a
case reports failed, the lane fails on the tally, which is the more dangerous direction because
nothing downstream would look again.

## Rigs take a heavy slot on the shared EC2 Mac (2026-10-26 20:30 UTC)

The Mac's heavy-job cap (`/usr/local/bin/arbos-heavy-slot.sh`, versioned at `scripts/runner/arbos-heavy-slot.sh`;
2 slots + 1 priority slot for hotfix pins) already covers the gate's Mac and iOS halves, the TestFlight archive
and the Mac release job. Rigs did not take part: at 20:23 UTC on 2026-10-26 one rig's simulator boot
(`launchd_sim` + healthd/healthappd/STExtraction/diagnosticd, ~490 % CPU for 90 s) on top of the two capped
builds took the 8-core box to load 263 and ssh timed out for every worker (same at 20:0x and at 13:00,
load 66–167). Rule, for every rig script under `/Users/Shared/arbos-*/` and every ad-hoc run on the box:

- Before `xcrun simctl boot`, `xcodebuild`, `cargo build`/`cargo run`, `swift build` or launching the Mac app
  from a rig: `slot=$(/usr/local/bin/arbos-heavy-slot.sh acquire <rig-name> --pid $$)`; after the heavy step
  (the simulator is up and the app is running, or the build finished): `/usr/local/bin/arbos-heavy-slot.sh
  release "$slot"`. Screenshots, clicks and `screencapture` on an already-running app need no slot — hold
  the GUI lock (`gui-lock.sh`) for those as before. A slot is for the burst, not the whole session.
- Builds in a rig run `nice -n 12` with `CARGO_BUILD_JOBS=4` (the gate does the same); a rig never needs the
  `--priority` slot — that is for a pinned hotfix.
- A rig that is refused for `WAIT_MAX_S` proceeds, as the gate does, and says so in its issue; a rig that
  holds a slot with no heavy process alive is stale and gets reclaimed.
- Shared simulators stay shut down when the rig is done (`xcrun simctl shutdown <udid>`): a booted idle
  simulator keeps a dozen daemons runnable.

## ssh to the gate Mac goes through the relay host (2026-10-26 23:40 UTC)

Port 22 on the EC2 Mac (13.217.52.163) is not open to the internet: its security group (`arbos-mobile`) allows ssh
only from a short list of fixed /32 addresses — the mobile-loop VM and its NAT set, a few named worker VMs, and the
relay host (204.12.171.6, "arboslife-jump"). Port 443 is public, which is why "443 answers but 22 times out" is the
symptom. Cloud agent VMs egress from rotating AWS addresses (eight different ones reached the relay host in six
hours on 2026-10-26), so adding your VM's address is pointless by the next session and opening 22 to all would be
the wrong fix. pf on the Mac is off and sshd listens on every interface — nothing on the Mac is in the way.

Rule: do not ssh to the Mac directly. Hop through the relay host, which every worker can already reach:

    ssh -o ProxyCommand="ssh -i <relay key> -W %h:%p const@204.12.171.6" -i <mac key> ec2-user@13.217.52.163

(or an equivalent `ProxyJump const@204.12.171.6` in a per-task ssh config under `/tmp`, never `~/.ssh`). The same
goes for `scp` and for VNC (:5900 is allowlisted the same way): tunnel it over the jump. A worker that needs the
list widened for a fixed address (a self-hosted machine, not a VM) asks the coordinator; the security group is
Jacob's and is not edited by workers.

## Two shared command runners on the EC2 Mac: `runner.sh` captures, `ax-runner.sh` clicks (2026-10-27)

No single TCC identity on the Mac has both grants: Terminal (which runs `/Users/Shared/arbos-run/runner.sh`)
has Screen Recording but Accessibility is denied; `sshd-keygen-wrapper` (which runs `ax-runner.sh`, started
via `launchctl asuser` from ssh) has Accessibility but no Screen Recording. So a rig step captures in
`runner.sh` and clicks in the ax-runner: `ax <command…>` queues one command there and returns its output and
exit code; `axbin/osascript` is a shim on `runner.sh`'s PATH that forwards every `osascript` call the same way.

- Never put `/Users/Shared/arbos-run/axbin` on PATH inside an `ax` command, a `cmd-*.sh` that calls `ax`, or
  anything that already runs in the ax-runner. 05:53–06:03 Oct 27: a rig's shoot.sh did, the osascript inside
  the ax-runner resolved to the forwarding shim and queued itself behind itself, and both shared runners were
  dead for 9 min. Inside an ax command call `/usr/bin/osascript` explicitly. The runner strips axbin from PATH
  and sets `ARBOS_AX_RUNNER=<pid>`; the shim and `ax` refuse to forward when that variable is set and run the
  command in place — a guard, not a licence.
- An ax job is killed after `AX_JOB_MAX_S` (600 s) with rc 124 and a line in `ax-runner.status` and syslog; a
  caller waits at most `AX_TIMEOUT_S` (600 s) for its result. A click step that needs longer is doing something
  wrong — split it.
- GPUI windows (the Arbos Mac app) ignore `System Events click at …` — that is an Accessibility *action*, and GPUI
  exposes no per-element AX tree, so the click lands nowhere. For a real click use `cgclick` (a CGEvent mouse
  click; `/Users/Shared/arbos-tools/bin/cgclick`, source `/Users/Shared/arbos-tools/src/cgclick.swift`) through
  the ax-runner: `ax /Users/Shared/arbos-tools/bin/cgclick <x> <y>`. CGEvent posting needs the same
  Accessibility grant, which is why it must go through `ax`, never directly from a `runner.sh` command.
- After a reboot both runners are gone: start `ax-runner.sh` via `sudo launchctl asuser 502 sudo -u arbosgui …`
  from ssh and `runner.sh` via `open /Users/Shared/arbos-run/start-runner.command` (Terminal identity).

## Standing rules that this does not change

- One issue per release; a hotfix marker (`[hotfix]`) is for P0s only.
- Mockups before any new UI; the `mockups-first` label marks items that wait for them.
- The malicious-intent screen, the kill switch and the CI gate stay as they are.
- Image rolls pause after a failed canary until a tagged fix lands or an operator lifts the hold.

Runbook (how the loop implements this): the project store's `docs/feedback-loop.md`.
