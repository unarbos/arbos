---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# plan tool shapes + refused-call rows — PR #113, branch `cursor/plan-tool-shapes-b027`

- `plan` reads: strings, `{section, text}`, `{label, target, readout}`, nested `{goal, children:[…]}` (parent → `##` section), a markdown checklist string; ops with aliases; missing `op` inferred. A call it cannot read errors with the accepted shapes (`notes::SHAPES`).
- Desktop: a failed tool call reads `<tool> <args> · refused: <first line>`; the fold body is the whole error. Kernel tools carry argument summaries (`plan set · 29 items`, `say to=root`, …).
- Seam: `Notes::set` keeps the page preamble (front matter + top link); #107's page guard removed from `resolve_write` (#103's fires).

## Scenario ideas

- Jacob's prompt on Fable 5.1 and gpt-5.4-mini: one `plan set` call each (verified). Try Opus / Sonnet / Kimi: any refusal now shows as `refused:` with the shapes; note which shape they send.
- Replay `{"op":"graph"}` (`ARBOS_PROVIDER=replay`) → row `plan graph · refused: …` and the body; never "missing text".
- A `set` on root's page keeps `+++ owner +++`, `# Notes`, and the `[project-context](docs/project-context.md)` line; `arbos-kernel check` (with #103) stays clean.
