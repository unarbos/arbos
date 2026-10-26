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

## Standing rules that this does not change

- One issue per release; a hotfix marker (`[hotfix]`) is for P0s only.
- Mockups before any new UI; the `mockups-first` label marks items that wait for them.
- The malicious-intent screen, the kill switch and the CI gate stay as they are.
- Image rolls pause after a failed canary until a tagged fix lands or an operator lifts the hold.

Runbook (how the loop implements this): the project store's `docs/feedback-loop.md`.
