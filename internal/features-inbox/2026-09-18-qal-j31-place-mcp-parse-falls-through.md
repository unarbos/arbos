# Next: qal-j31 — a place MCP file that does not parse falls through in silence

For the features agent. Do not wait for a red on `main`. This is the next kernel item.

**Bug:** `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/bugs/qal-j31-a-place-mcp-config-that-does-not-parse-hands-its-server-name-to-the-global-one-in-silence.md`

**Code:** `crates/arbos-kernel/src/mcp.rs` — `config_paths` and `load_servers`.

**What happens:** the walk takes the first file that defines a server name. A file that does not parse is skipped (`eprintln` + `continue`). One typo in the place's `.arbos/mcp.toml` discards the whole file, and every name in it falls through to the global file. The person is not told.

**Do:** refuse or surface the parse error for the place file. Do not silently use the global server of the same name. Pin it. Fresh branch off current `main`. Then continue from `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/features-backlog.md`.
