---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-16 follow-up: `read` / `tail` / `list` frames

Branch `cursor/frame-read-tail-b027` (on `cursor/release-integration-52cd`). The design's RPC option for remote reads: `{"type":"read","path":"agents/root/plan.md"}` → `file {text, size, truncated}`; `{"type":"tail","path":"agents/root/transcript.jsonl","from":N,"limit":L}` → `chunk {from, to, size, text}` cut at the last newline unless it reaches the end; `{"type":"list","path":"agents"}` → `listing {entries:[{name, dir, size, modified}]}`. Paths are relative to `.arbos/`; answered on the asking connection only; `reader` role may use them. `access.toml` is refused and hidden from the top-level listing.

Verified from the network as a reader: list, read, tail with line cut, tail past the end (empty, `to == size`), `..` refused, absolute refused, a symlink `.arbos/etc-link → /etc` refused ("leaves .arbos/"), folder read refused with "use list", `user` still refused for readers.

Attack surface: a path with a NUL or very long name; `list` of a folder with 10k entries (no cap today); `tail` with `limit` 1 and a line longer than that (returns empty text, `to == from` — the client must grow `limit`); a transcript line with invalid UTF-8 (lossy); symlinked *directory* inside `.arbos/` pointing inside `.arbos/` (allowed); `read` of `kernel.json` and `kernel.log` (served — fine?); `worktrees/` under `.arbos/` holds project checkouts, so `read worktrees/<id>/src/main.rs` works — is that intended for a reader role? Say if the product wants `worktrees/` excluded.
