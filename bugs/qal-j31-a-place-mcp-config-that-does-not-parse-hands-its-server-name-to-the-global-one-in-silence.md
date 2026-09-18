# qal-j31 — a place MCP config that does not parse hands its server name to the global one, in silence

- **status**: **closed — fixed on `main` by [#613](https://github.com/unarbos/arbos/pull/613) (`23ef527c`), re-checked 2026-09-18 12:49. The residual is closed too by [#644](https://github.com/unarbos/arbos/pull/644) (`f97bb348`), re-checked 2026-09-18 13:14.
- **found**: 2026-09-18 08:22, by walking the kernel's first-match readers rather than by a break
- **kernel**: `arbos-kernel 0.2.0 f80f0b663bac protocol 1`
- **code**: `crates/arbos-kernel/src/mcp.rs:93` (`config_paths`) and `:116` (`load_servers`)
- **control**: `fm-02-a-place-mcp-file-that-does-not-parse-is-said-and-does-not-hand-its-name-away` (renamed with the contract)
- **rollout**: `20260918T082246Z-fm-02-…`

## What happens

`load_servers` walks four locations in order and the **first file to define a server name keeps
it**:

1. `.arbos/mcp.toml` — the place's own
2. `.cursor/mcp.json`
3. `.mcp.json`
4. `$XDG_CONFIG_HOME/arbos/mcp.toml` — the person's global

A file that does not parse is skipped and the walk continues:

```rust
let file = match parsed {
    Ok(f) => f,
    Err(e) => { eprintln!("mcp: {}: {e}", path.display()); continue; }
};
```

So one wrong character in the place's config does not disable one server — it discards the whole
file, and every name in it falls through to whatever the next location says.

## Demonstrated, not argued

Two arms, identical but for the global file, same broken `.arbos/mcp.toml` defining `notes`:

| arm | kernel stderr |
|---|---|
| global config also defines `notes` | `mcp: …/.arbos/mcp.toml: TOML parse error at line 2, column 11`<br>`mcp: notes: MCP server notes sent no reply to tools/list` |
| no global config | `mcp: …/.arbos/mcp.toml: TOML parse error at line 2, column 11` — and nothing else |

The second line appears only when the global config exists, so the `notes` server the kernel goes
on to start can only have come from it. The place's own `notes` never existed as far as the kernel
is concerned. **The agent is handed a different server under the name its author configured.**

## Why it is worth filing when the skip is deliberate

The code says what it intends — *"Config trouble is reported on stderr; the rest still loads"* —
and loading the rest is the right call. The defect is the reporting, and it is the
who-reads-this-message question:

- the kernel writes to stderr
- the desktop routes the kernel's stdout and stderr to `.arbos/runtime/kernel.out.log`, described
  in its own comment as *"process facts, never part of the `.arbos/` record"*
  (`desktop/src/kernel.rs:2789`)
- the window never shows that file

So the only record of "your config has a typo and someone else's server answered to its name" is
in a log written specifically to be outside everything a person reads. Root's transcript carries
nothing: `notices_about_mcp: []`.

The failure is quiet in both directions. If a global config exists the person silently gets the
wrong server. If it does not, they silently get no server, and the agent simply lacks a tool —
which reads as the model choosing not to use it.

## The fix this needs

Say it where its author will see it. `uw-02` already established the shape for exactly this class
in #444: a `failed` notice on root's transcript, once per start, naming the file and what to do.
The same treatment here — name the path, the parse error, and which server names were affected —
would close it.

## The seventh reader

The unchecked-writes audit enumerated six first-match readers and cleared five of them: the
history lookup, the checkpoint sidecar, the roster files, the leash pointer, the legacy
`kernel.json`, with only the held-record loader reading the wrong copy. `mcp::load_servers` is not
on that list.

That is the argument for building the `fm-*` family as a standing property rather than a set of
one-off checks, which was the reasoning given when the family was proposed: *the reasons are
today's, and the next reader added will not have been checked by anyone.* This one was already
there and unchecked. `fm-02` now holds the property.

## Closed: re-checked against #613

The features agent's read is `internal/qa/inbox/2026-09-18-qal-j31-mcp-parse-said.md`. `mcp::load`
now returns the servers **and the problems**: a place file that does not parse is said as a notice
on root's transcript, and it blocks the machine's file, so the name cannot be served by the
machine's server of the same name.

Re-checked on `arbos-kernel 0.2.0 a8678ac16636 protocol 1` — today's `main`, which carries #613 —
against `42cb9751ace8`, which does not. Same staging both times: one unclosed array in
`.arbos/mcp.toml`, and a valid `notes` in `$XDG_CONFIG_HOME/arbos/mcp.toml`.

| | `42cb9751ace8` (before) | `a8678ac16636` (with #613) |
|---|---|---|
| `MCP:` notice on root's transcript | **none** | the notice, naming the file and the parse error |
| the machine's `notes` server started | — | **not** started |

The notice reads: *"MCP: .arbos/mcp.toml does not parse (TOML parse error at line 3, column 10 …
invalid array, expected `]`). Its servers are off, and the machine's own MCP file was not used in
its place…"* — the file, the fault, and what it cost, on the transcript rather than in a log the
window never shows. That is the whole of what this file asked for, so it closes.

## The residual, and its close: #644

#613 blocked the **machine's** file only. The place's own later files — `.cursor/mcp.json`,
`.mcp.json` — were still read, and could hand the same name a different server while the notice
said "its servers are off". Reproduced here on `a8678ac16636` in two arms differing only in that
one file: with `.cursor/mcp.json` offering `notes` a server started; without it none did. So the
machine's file was genuinely blocked, and the later place file was the remaining route.

[#644](https://github.com/unarbos/arbos/pull/644) landed as `f97bb348`. `load_from` now `break`s at
the first place file that does not parse, so nothing after it is read, and `problem_notice` widened
from *"the machine's own MCP file was not used in its place"* to *"no MCP file after it was read in
its place — not the place's other files, not the machine's own"*.

Re-checked, same staging, both arms:

| | `a8678ac16636` (#613 only) | `f97bb3487540` (with #644) |
|---|---|---|
| a `notes` server started | **yes** — `mcp: notes: …` | **none** |
| notice names the files after the broken one | no | yes |

`fm-02` passes on the #644 head and breaks twice on the one before it, so it fails on the unfixed
build and passes on the fixed one in both halves of the contract. `qal-j31` is closed entire.

The self-gate is gone. While #644 was open, `fm-02` gated that third check on the product's own
claim rather than standing a red for an unmerged fix; with the fix on `main` it is a plain
assertion. Worth noting the gate would **not** have armed itself as written: it looked for the word
"later" and #644's wording is "after it" — so the device was right in principle and wrong in its
trigger, and a merged PR needed a human read either way. A self-gate on a product's wording is a
guess about a sentence nobody had written yet.

The second assertion above is new and came out of that: the notice must describe the whole block it
performs, not less. On `a8678ac16636` it names only the machine's file, which would let a person
believe a later place file might still have taken the name.
