# qal-j09: one unreadable read of the project page, and the next `plan` call replaces the page with an empty one — reporting success

- Measured at: `main` @ `7f6a6b9a` (`arbos-kernel 0.2.0 7f6a6b9a06bc`); replay provider, no model. Code read at `main` @ `0f2a8bc6`.
- Family: qal-j08's — a read that fails is treated as *nothing* rather than *unknown*, and a later write acts on it with confidence. Here the write is the rewrite of `.arbos/notes.md` (the project page: goal, checklist, notes) and of every agent's own checklist page.
- Severity: **high, destructive.** The project page is the coordinator's memory of what the project is for and what is in flight; on this team it also lives on a network mount that answers partially (six store episodes today). One failed read at the moment of a `plan` call and the page is gone, with `Set 1 item(s).` as the only word said. `archived.md` gets nothing (the overflow was empty), so nothing is moved — it is dropped.
- Scenario: `sw-01-unreadable-notes-page-is-rewritten-empty`; rollout `internal/qa/rollouts/20260917T044218Z-sw-01-…`: 293-byte page → 36 bytes (the one new item).

## Repro

`.arbos/notes.md` with a goal, three checklist items and two lines of notes. Kernel running. Make the file unreadable for one call (`chmod 000` is the injector; in life it is EIO, a lock, a partial view on a mount, a file mid-replacement by another writer). The model calls `plan {items: ["- [ ] Add perimeter() tests — ready"]}`.

After: `notes.md` = `- [ ] Add perimeter() tests — ready\n` (36 bytes). Tool result: `Set 1 item(s).` No error anywhere.

## Expected

A page that could not be read is not rewritten. The `plan` call fails with the reason (`could not read .arbos/notes.md: permission denied — nothing written`), the transcript carries a failed notice, and the page keeps its bytes. If the page is genuinely absent, empty is the right start; absent and unreadable are different answers and must stay different.

## Actual

`arbos_core::notes::load` → `Notes::parse(&read_to_string(path).unwrap_or_default())` — any `Err` becomes `""`. `save_path` then writes the rendered (empty + new item) page to a temp file and `rename`s it over the original; rename does not care that the original was unreadable. Same shape, same crate, in `notes::read_path`.

## Suspected location

- `crates/arbos-core/src/notes.rs::load` / `read_path`: return `Result`; treat `NotFound` as an empty page and every other error as an error the caller must surface. Callers: the `plan` tool, the project-page updater, the archive.
- The same pattern, ranked below by what acts on it: `arbos-engine/src/tools/memory.rs::load` (`remember` rewrites `memory.md` from the loaded text with `fs::write` — a transient read failure wipes memory), `arbos-kernel/src/hooks.rs::notify_user` (`user.md` rewritten from `unwrap_or_default` — the user's inbox log truncated to one line), `arbos-core/src/files.rs::init_arbos_repo` (`.arbos/.gitignore` merged from empty — a hand's extra lines lost), `arbos-kernel/src/plan.rs` `meta.toml` (a run's record rewritten as its tail — misreport only).

## Fix

Not started (features agent). Regression check: `sw-01` (page bytes unchanged after the call; the tool call carries an error).
