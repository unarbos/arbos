# qa-039: `store put` on a conflict prints no current hash; a peer's refusal on a root-owned page says `say to=root` instead of `say to=<machine>/<project>/root`

- Feature: mesh federated store (`arbos://machine/project/path`), `main` @ `b49e6163`
- Severity: low. Both are wording/plumbing at the edge; the rules themselves hold (see the scenario: read, ls, shared put, compare-and-swap conflict keeps the file, right hash lands, root-owned page refused and unchanged, `..` refused, unknown machine fails fast).
- Scenario: `fs-01-federated-store-read-write-cas` (check `fs-01-cas-no-current-hash`); rollout `internal/qa/rollouts/20260916T090700Z-fs-01-federated-store-read-write-cas/`

## 1. Conflict without the current hash

`arbos-kernel store put arbos://qa-b/beta/docs/from-a.md file --base <stale>` prints
`Error: docs/from-a.md: conflict — the file changed since you read it; read it again and redo the edit`.
The `written` frame carries `hash` (the wire doc: "on a refusal or a conflict … `hash` is still the current file's, so the writer can re-read"), but the CLI drops it. A writer scripting a compare-and-swap loop has to do a second `store read` to learn the hash it could have been told. Print it: `… (current sha256 8b03030f…)`.

## 2. The peer refusal names a target the peer cannot use

Writing `arbos://qa-b/beta/.arbos/notes.md` from node A is refused with the *local* text: "…owned by the main chat (root); keep your own checklist with the plan tool and propose the change with `say to=root`". From another node, `say to=root` is A's own root. `files.rs` says the peer should be pointed at `say to=<machine>/<project>/root`; the `put` path uses `store::REFUSAL` instead of a peer-aware line.

## Suspected location

`crates/arbos-kernel/src/store_cmd.rs` (print `hash` on error) and `crates/arbos-kernel/src/files.rs` `put` (peer refusal text with the address's machine/project).
