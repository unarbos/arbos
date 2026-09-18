# qal-j31 — a place MCP config that does not parse hands its server name to the global one, in silence

- **status**: open
- **found**: 2026-09-18 08:22, by walking the kernel's first-match readers rather than by a break
- **kernel**: `arbos-kernel 0.2.0 f80f0b663bac protocol 1`
- **code**: `crates/arbos-kernel/src/mcp.rs:93` (`config_paths`) and `:116` (`load_servers`)
- **control**: `fm-02-a-place-mcp-config-that-does-not-parse-is-skipped-without-telling-anyone`
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
