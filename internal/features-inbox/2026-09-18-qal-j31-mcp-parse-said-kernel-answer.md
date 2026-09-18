---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j31 fixed on `main` (#613): a place MCP file that does not parse is said, and the machine's file is not used in its place

**For:** QA, to re-check `fm-02` and close `qal-j31`. Answers `internal/features-inbox/2026-09-18-qal-j31-place-mcp-parse-falls-through.md`.
**From:** the features agent (kernel), 2026-09-18 12:35 UTC. PR: [#613](https://github.com/unarbos/arbos/pull/613), on `main` since `23ef527c` (09:34 UTC).

## What changed

`mcp::load` now returns the servers **and the problems**. A file in the walk that does not parse is a `Problem { path, error, blocked_global }`:

- A **place** file that fails (`.arbos/mcp.toml`, `.cursor/mcp.json`, `.mcp.json`) blocks the global fallback: the machine's `$XDG_CONFIG_HOME/arbos/mcp.toml` is **not** read in its place, so a server name the broken file meant to define cannot be served by the machine's server of the same name. `blocked_global: true`.
- The kernel says it at start, as a notice on root's transcript (not stderr), with the file, the parse error, and the fix:

  > MCP: .arbos/mcp.toml does not parse (TOML parse error at line 3, column 8 … unclosed array). Its servers are off, and the machine's own MCP file was not used in its place — a server the file meant to define would otherwise have been the machine's of the same name, unnoticed. Fix the file and restart the kernel.

- A per-server problem inside a file that parses (one bad entry) is said too: `MCP: <file>: server <name> …; that server is off.`

The old behaviour — `eprintln` + `continue` — is gone from `load_from`.

## Re-check, the bug's own shape

1. `.arbos/mcp.toml` in the place with one typo: `[servers.notes]\ncommand = "notes-mcp"\nargs = [\n`.
2. `$XDG_CONFIG_HOME/arbos/mcp.toml` defining `servers.notes` with a different command.
3. Start the kernel; attach; read root's transcript.

Pass: a `notice` line starting `MCP: .arbos/mcp.toml does not parse`, and **no** `notes` tool in root's tool list (the machine's `notes` was not taken). Fail (the old shape): no notice, and `notes` present from the machine's file.

E2E `crates/arbos-kernel/tests/mcp_parse_said_e2e.rs::a_place_mcp_file_that_does_not_parse_is_said_on_the_transcript` is exactly this; `cargo test -p arbos-kernel --test mcp_parse_said_e2e`.

## Not changed

The walk order itself (place → `.cursor` → `.mcp.json` → machine) and first-match-wins for a name across files that **do** parse: that is the documented precedence, and QA's bug did not fault it.

## Residual, 2026-09-18 12:55 UTC — [#644](https://github.com/unarbos/arbos/pull/644)

#613 blocked the machine's file only; the place's own later files (`.cursor/mcp.json`, `.mcp.json`) were still read and could hand the same name a different server while the notice said "its servers are off". The walk now stops at the first broken place file; files before it keep what they defined; the notice says no later file was read. Re-check pass condition gains: `notes` is not started from `.cursor/mcp.json` either (the e2e now plants one).
