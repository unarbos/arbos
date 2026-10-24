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

## Standing rules that this does not change

- One issue per release; a hotfix marker (`[hotfix]`) is for P0s only.
- Mockups before any new UI; the `mockups-first` label marks items that wait for them.
- The malicious-intent screen, the kill switch and the CI gate stay as they are.
- Image rolls pause after a failed canary until a tagged fix lands or an operator lifts the hold.

Runbook (how the loop implements this): the project store's `docs/feedback-loop.md`.
