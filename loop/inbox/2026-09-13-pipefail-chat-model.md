# J-01 pipefail for jobs + J-05 per-chat model memory — QA note (features agent, 2026-09-13)

Branch `cursor/pipefail-chat-model-b027`, base `rust`. Both found by Jacob on his Mac.

## J-01 — plan shell nodes and bash jobs

- Every job's wrapper shell now runs with `set -o pipefail` (when the shell supports it; `sh -c` on dash does not — the wrapper probes and uses it when available), so `curl … | jq …` fails when `curl` fails.
- A shell node with a `report` whose command exits 0 but prints nothing is now a **failure** (`exit 0, no output`): the model is woken as for a failed command, nothing is delivered. Nodes without a `report` keep exit-code semantics (a silent success is fine for `make`).
- A `report` template must contain `{output}`; `plan add` rejects one that does not (`report must contain {output}`), and the CONTRACT line says so. Existing plan files with old templates keep working (the check is at add time).

Attack ideas:
1. `false | true` as a shell node: exit 1 now (was 0).
2. `echo` with `report: "BTC: {output}"`: delivered "BTC: " — empty output is a failure only when the output is empty *after trimming*; `echo` prints a newline → empty → failure. Check the wording of the wake.
3. A node with `report` and a command that prints only to stderr: stderr is in the journal (merged), so it counts as output — decide if that is wanted.
4. macOS `/bin/sh` is bash-in-sh-mode: `set -o pipefail` works there; Debian `dash` rejects it → the probe must not print an error into the journal.
5. Sandbox (#52) + pipefail: the `set -o pipefail` goes inside the same script; nothing else changes.
6. `plan add` with `report: "done"` (no `{output}`): rejected with the message; the model must rewrite.

## J-05 — per-chat model

- The kernel already saved `SetModel` into `agent.md` and used it for turns; the desktop never read it back, so after a reopen the chip showed the config default and a model picked offline was re-sent over whatever the agent had.
- Now: the desktop's transcript replay reads `agent.md` → `model` (unless `inherit`) into the chat; at go-live the chat's model is only pushed to the kernel when the agent has none; the chip shows the agent's model.

Attack ideas:
7. Pick model A in chat 1, model B in chat 2, restart the desktop: each chip shows its own; `agent.md` of each says so.
8. Pick a model, then change `config.toml` `model`: the chat keeps its own; a new chat gets the new default.
9. A child agent (`model: inherit`): chip shows the default; `turn.rs` resolves inherit to the config model — unchanged.
10. Remote place: `agent.md` is not readable locally → the chip falls back to the default; the kernel still uses the saved model. Note.
