---
cursor:
  subagentId: "bc-fe947dc6-3057-5855-9e6e-2e28531794d6"
---

# Project store + coordinator directive + Project panel — PRs #103 (kernel, on #107) and #105 (desktop, on #71)

What changed, in one breath: a fresh place now has Cursor's Agent Store shape under `.arbos/` — `notes.md` (the project page, root-only), `docs/project-context.md` (goals/constraints/decisions/resources; `GOALS.md` is a symlink to it for one release), `docs/`, `internal/`, `media/`, `archived.md`. Root (`role = "coordinator"`) gets `write`/`edit` back but only inside that store; it runs the project the way Cursor's coordinator does (`COORDINATOR_CONTRACT` in `prompt.rs`). `spawn` takes a six-field kickoff (`name`, `task`, `read_first`, `do`, `rules`, `output`, `report`) and a `name` that becomes a hyphenated id (`write-river-poem`). The panel's "Goals" is now "Project": a Context row, then `notes.md` drawn like Cursor's Projects page, then Standing rows from `agents/*/subscriptions/*.toml`.

## Scenario ideas

- Fresh place: `.arbos/{notes.md,archived.md,docs/project-context.md,docs,internal,media}` exist after the first kernel start; `GOALS.md` is a symlink whose content equals `docs/project-context.md`; `arbos-kernel check` clean.
- Old place with a real `GOALS.md` (pre-#103 kernel): next start moves it to `docs/project-context.md`, leaves the symlink, keeps the text; `check` on the pre-start state warns "old layout".
- Child `write path=.arbos/notes.md` → refused ("owned by the main chat"); same for `docs/project-context.md`, `archived.md`, `GOALS.md`. Child `write path=.arbos/docs/design.md` → allowed.
- Coordinator root `write path=src/main.rs` → refused ("as coordinator you write only the project store"); `write path=.arbos/docs/x.md` → allowed; `bash` is not in its tool list.
- Multi-goal prompt to a coordinator root → two `spawn` calls in one step, each with the six fields (`turns/t0001/cause.md` of each child starts with `Read first:`); ids are hyphenated words; `.arbos/notes.md` gets one `- [ ] [label](agents/<id>) — readout` per goal, no bare paths as labels; on a `[done]` the item flips to `- [x] [label](docs/<file>) — readout`, checked items last in their section.
- Kernel `changed` frame for `notes.md` arrives right after root's turn ends (before the next 1 s watch tick); a second one from the watch within a second is allowed.
- `check` warnings: an item without a checkbox, an item without a leading `[label](target)`, five `<tldr>` bullets, four checked items in one section, an open item after a checked one; each names the line. Not errors — fixtures still pass.
- Panel (#105, Linux driver under Xvfb): section reads "Project"; "Context" row opens `docs/project-context.md` as a doc surface; a page item's label is the only clickable part; a `docs/…` target opens a doc surface, an `agents/<id>` target opens that worker's chat, an `https://` target calls the browser; readouts are dim under the label; front matter and the `#` title never show; template-only page shows "Nothing on the project page yet. Start the page…" which puts a prompt in the main chat's composer.
- Standing rows: a subscription with `deliver_to = "user"` or the default shows under Project → Standing with `every … · next HH:MM`; the kernel's weekly `git gc` (`deliver_to = "none"`) does not.
- Remote place (attached over the hub): the page updates on the kernel's `changed` frame, no file watch there.

## Known / not done

- Remote `spawn host=…` ignores `name` (id from the brief's first words).
- `.arbos/notes.md` has two guards after the rebase (#103 `store::is_root_owned`, #107 `notes::is_project_page`); the first fires, both messages mention the project page.
- The desktop's `cargo test --lib` on the #71 base does not compile (`WORK_ROWS`/`work_bits` in `transcript.rs` tests) — pre-existing; the new `store_view` tests could not run.
- The chat's Plan strip (features' synthesised rows) shows the item's raw `[label](target)` markdown; the panel strips it. Theirs to tidy.
