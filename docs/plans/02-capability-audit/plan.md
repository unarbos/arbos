# Capability audit — arbos pi agent (OpenRouter live)

Date: 2026-06-08. Model: `google/gemini-2.5-flash` via OpenRouter (`doppler arbos/dev`).

This audit exercised the **arbos pi coding agent** (`cmd/arbos`) against thirteen
capability dimensions. Each item was run live unless noted. Gaps are prioritized
for follow-up phases.

## Results

| # | Capability | Status | Evidence |
|---|------------|--------|----------|
| 1 | Web search | **FAIL** | Agent has no search/fetch tool; model refused: *"I do not have access to web search tools."* Tool inventory: `ls read find grep write edit bash delegate start_coding_session` only. |
| 2 | Write and execute code | **PASS** | Wrote `calc.py`, ran `python3 calc.py`, got `4`. |
| 3 | Answer questions | **PASS** | "Capital of France" → `Paris` with no tools. |
| 4 | Plan and execute | **PASS** | Multi-step: write `plan.txt` + `result.txt`, read both, replied `DONE`. |
| 5 | Tool use | **PASS** | Chained `ls` → `write` → `read` in one turn. |
| 6 | Terminal | **PASS** | `bash uname -s && pwd` → `Linux` + cwd. |
| 7 | Interact with machine | **PASS** | Filesystem R/W, subprocess env, network via bash (see #13). |
| 8 | Update code in repo | **PASS** | Edited copy of `main.go` in temp dir; wrote `.cap-audit-marker` in real `arbos/` tree. |
| 9 | Long context | **PARTIAL** | `find` + `read` on 30 large files works. `CompactionPolicy` exists but **not stress-tested** at window overflow. |
| 10 | Memory | **FAIL** | No `memory` tool. Model called `delegate` with `tools:["memory"]` — child replied but **nothing persists** semantically. File notes + SQLite event log work; no recall layer. |
| 11 | Skills | **PARTIAL** | `.arbos/skills/*/SKILL.md` loads into prompt index. Model **did not auto-read** skill until explicitly told; after `read` → correct codeword. Matches pi (skills are opt-in via read). |
| 12 | Run commands | **PASS** | `bash` runs arbitrary shell (curl, python, jq). |
| 13 | RPC / HTTP calls | **PARTIAL** | `curl` via bash works (POST to httpbin → `True`). Control seam works when stdin stays open (e2e pattern). **Piped one-shot serve** exits on EOF and cancels in-flight turns. MCP + extensions **built but not wired** into entrypoints. |

### Cursor agent (this IDE session)

For comparison: the Cursor agent running this chat **can** do all thirteen in
this environment (web search tool, terminal, MCP browser, file edits, skills
rules, long context, etc.). The gaps above are **arbos-specific**, not host-IDE
gaps.

## Holes (ranked)

### P0 — user-visible capability gaps

1. **No web search / fetch tool** — cannot answer time-sensitive or external facts without bash hacks; no structured HTML/JSON fetch with size limits and auth.
2. **No persistent memory** — memory provider was removed during pruning; `delegate` with fake tool names misleads the model.
3. **MCP not wired** — `internal/mcp` client+server exist; `piwire` and entrypoints never spawn or attach MCP servers, so external tool ecosystems are unreachable.
4. **Extensions not wired** — `internal/extension` is staged (ADR-0030); no host loads extensions, so runtime tool/command registration is dead code from the user's perspective.

### P1 — reliability and UX gaps

5. **Control seam EOF race** — when stdin closes after sending intents, `Serve` returns immediately (`lines` channel closed) and `teardown()` cancels the session before the LLM finishes. Fork over pipe hit `context canceled` on persist. Fix: drain in-flight turns after EOF (configurable grace) or document "keep stdin open" (e2e already does the latter).
6. **No CLI session resume** — `-prompt` always starts a new session. Resuming requires `-serve` + `open` with `session_id`. Add `-session <id>` to one-shot mode.
7. **Skills discovery not verified in e2e** — loader works manually; no automated proof that `<available_skills>` reaches the model and that read-on-match works without explicit nudge.
8. **Compaction not live-tested** — policy + summarizer implemented; no audit script forces overflow and asserts `CompressionPayload` in SQLite.

### P2 — polish and parity

9. **HTTP tool vs bash** — bash works but is unconstrained (no URL allowlist, no response truncation policy, model may hallucinate on failed curls).
10. **RPC surface incomplete vs pi** — `set_model` / `compact` exist as intents but no ergonomic RPC frames or slash commands documented for headless clients.
11. **TUI not in audit/e2e** — `cmd/arbos-tui` built but not exercised in `scripts/e2e-live.sh`.
12. **Static gate drift** — `gofmt` failure on `cmd/arbos/main.go` comment block blocked full e2e run during audit.
13. ~~**Delegate tool list is not validated** — `tools:["memory"]` silently runs child with empty grant instead of erroring.~~ **RESOLVED** — pi's child-engine factory rejects unknown grant tools (`internal/agent/pi/agent.go`); `tools:["memory"]` now returns `grant tool "memory" is not available to delegated agents` to the model.

## Fix plan

### Phase A — Close the obvious holes (P0)

**A1. Web fetch tool** (`fetch` or `web_fetch`)

- Add read-only tool to `codingspec`: GET/POST, timeout, max bytes, optional headers via secret refs.
- Truncate + content-type handling (text/json/html strip).
- Register in pi toolset; add to fake provider scripted responses.
- Live e2e: fetch a stable URL, assert body fragment in tool result.

**A2. Memory port restoration (minimal)**

- Reintroduce a `memory` tool backed by SQLite or project-scoped JSON under `~/.config/arbos/memory/` + `.arbos/memory/`.
- Inject latest memory segments as `ContextPayload` (kernel already renders `<<memory>>` blocks).
- Remove or guard `delegate` allowlist so unknown tool names error at call time.

**A3. Wire MCP into piwire**

- Config: `ARBOS_MCP_CONFIG` JSON or `.arbos/mcp.json` listing server commands.
- On `Assemble`: spawn servers, `ListTools`, register dynamic tools on the coding registry.
- Lifecycle: teardown on session end; 60s call timeout already in client.
- e2e: fake MCP server script returning one tool; live optional.

**A4. Wire extension host**

- `piwire.Assemble`: optional `Extension ...Extension` list; `NewHost` merges tools into registry.
- Ship one sample extension (e.g. `fetch` if not in core) to prove the seam.
- Document build-tag registration pattern.

### Phase B — Reliability (P1)

**B1. Control seam graceful shutdown**

- After stdin EOF, continue reading events until turn completes or `ARBOS_SERVE_DRAIN_TIMEOUT` (default 120s).
- Only then `teardown()`. Add unit test simulating piped client.

**B2. One-shot session resume**

- `cmd/arbos`: `-session <id>` resumes existing log from `-db` instead of new session.
- e2e: write file in turn 1, resume in turn 2, assert read.

**B3. Capability audit script**

- Add `scripts/capability-audit.sh` (or extend `e2e-live.sh`) covering all 13 dimensions with PASS/FAIL output.
- Include skills auto-load check, compaction stress (synthetic large context), control seam with open stdin.

**B4. Compaction live proof**

- Script appends synthetic turns until `ContextPolicy.ShouldCompact`; assert compression event in store and smaller projected context.

### Phase C — Parity and polish (P2)

**C1. RPC ergonomics** — document + optional control frames: `set_model`, `compact` as top-level frame types (thin wrappers over intents).

**C2. TUI smoke** — `make e2e-tui` headless or `expect`-less: build + `--help` + one scripted input.

**C3. Delegate grant validation** — ~~reject unknown tool names in `Grant.Tools` at registration or call time.~~ **DONE** — enforced in pi's child-engine factory (`internal/agent/pi/agent.go`).

**C4. Fix gofmt / static gate** — normalize `cmd/arbos/main.go` comment formatting so `e2e-live.sh` passes.

## Suggested execution order

```
A1 fetch → A2 memory → B2 session resume → B1 seam drain → A3 MCP → A4 extensions
         → B3 audit script → B4 compaction proof → C1–C4
```

Estimated effort: **A1–A2 + B2–B3** (~2–3 days) gets a credible "yes" on items 1, 10, 11, 13 for most users. **A3–A4** unlocks ecosystem extensibility. **B4** closes the long-context question definitively.

## Immediate workarounds (today)

| Need | Workaround |
|------|------------|
| Web facts | `bash` + `curl` (fragile; model may hallucinate on errors) |
| Memory | Write notes to a file in workspace; re-read next session |
| Session continue | `-serve` with open stdin + `open`/`session_id` |
| External tools | None until MCP wired |
| Skills | Put skills in `.arbos/skills/`; prompt model to check `<available_skills>` |

## Verification command

```bash
doppler run --project arbos --config dev -- \
  env ARBOS_MODEL=google/gemini-2.5-flash ./scripts/e2e-live.sh
```

After Phase B3:

```bash
./scripts/capability-audit.sh   # all 13 checks, exit non-zero on FAIL
```
