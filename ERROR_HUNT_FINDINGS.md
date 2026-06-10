# arbos error hunt — findings (2026-06-10)

Layered sweep: toolchain → static analysis → tests under race → vuln scan → targeted manual review.

## Automated layers (clean or benign)

| Check | Result |
|-------|--------|
| `go build ./...` | clean |
| `go vet ./...` | clean |
| `gofmt -l .` | clean |
| `go test -race ./...` | **all pass, no data races** |
| `govulncheck ./...` | 0 reachable vulns (1 in a required-but-uncalled module) |
| `golangci-lint run ./...` | 3 issues, all low/benign (below) |

### Lint items (low severity, no action required)
- `internal/tool/codingspec/checkpoint.go:126` — `idx.Close()` unchecked. Fine: temp file is removed on the very next line.
- `internal/tool/codingspec/checkpoint.go:132` — `defer os.Remove(idxPath)` unchecked. Fine: best-effort temp cleanup.
- `internal/engine/engine.go:442` — staticcheck S1016 (cosmetic): convert `SteerIntent`→`PromptIntent` instead of struct literal.

## Manual review findings

### HIGH

**H1 — `turnDone` channel leak on back-to-back turns** · `internal/control/server.go:118-131`
`signalTurnStart()` overwrites `turnDone` with a fresh channel without closing the previous one. `signalTurnEnd()` only fires from the pump on `TurnComplete`/`ErrorEvent`/`Interrupted`. If a second turn is signalled before the first's terminal event arrives, the old channel is never closed, so any `waitDrain()` holding it blocks until the drain timeout instead of real completion.
*Fix:* in `signalTurnStart`, close any existing non-nil `turnDone` before replacing it (or refuse to start a new turn while one is outstanding).

### MEDIUM

**M1 — `teardown()` does not close `turnDone`** · `internal/control/server.go:104-165`
On `switch_session`/`fork` mid-turn, `teardown()` cancels the session and the pump exits its `range` loop without ever seeing a terminal event, leaving `turnDone` open. A later `waitDrain()` blocks on a stale channel for a dead session.
*Fix:* call `signalTurnEnd()` inside `teardown()` after `pump.Wait()`.

**M2 — `ClaimOutbox` does not check `rows.Err()`** · `internal/sqlite/outboxstore.go:50-64`
The read loop closes `rows` and checks the Close error, but never calls `rows.Err()`. A row-level read fault mid-iteration makes `rows.Next()` return false, the loop exits as if complete, and the (truncated) message set is then marked delivered — **silently dropping the un-iterated messages.** Every other reader in the package checks `rows.Err()`; this is the lone omission.
*Fix:* after the loop, before relying on `msgs`, add `if err := rows.Err(); err != nil { return nil, fmt.Errorf("claim outbox: %w", err) }`.

**M3 — blocking `conv.Send` can wedge the request loop** · `internal/control/server.go:262-264`
`InterruptIntent`/`SteerIntent` use the blocking `conv.Send`, on the single request-reading goroutine. If the actor is not draining intents (e.g. stuck on a slow events consumer), the whole frame loop blocks and no further frames — including a follow-up interrupt — are read. The "interrupts drain promptly" claim is an assumption, not a guarantee under a stalled pump.
*Fix:* `select { case c.intents <- i: case <-sctx.Done(): }`, or a context-aware send.

### LOW

**L1 — `dispatch.go` semaphore acquire is not ctx-aware** · `internal/engine/dispatch.go:255`
`sem <- struct{}{}` blocks without watching `ctx.Done()`. Within an oversized wave (>8 calls, `maxParallelTools`), interrupt latency is bounded by the slowest in-flight tool rather than being prompt. Not a deadlock (tools honor ctx), just coarser cancellation than the sequential path.
*Fix:* `select { case sem <- struct{}{}: case <-ctx.Done(): }`.

**L2 — `SetPlanNodeStatus` swallows `RowsAffected` error** · `internal/sqlite/plannodes.go:139`
`rows, _ := res.RowsAffected()` — inconsistent with every sibling method, which propagate it. If `RowsAffected` errored, `rows` is 0 and the method wrongly reports "not found" instead of the real driver error.
*Fix:* capture and return the error as the siblings do.

**L3 — migration relies on string-matching `"duplicate column"`** · `internal/sqlite/store.go:196`
Idempotent re-migration of the `cmd`/`wake`/`notify` ALTERs depends on `strings.Contains(err.Error(), "duplicate column")`. Works with the current modernc.org/sqlite message, but fragile to driver wording changes — a reword would turn idempotent re-migration into a hard failure.
*Fix:* probe `PRAGMA table_info(plan_nodes)` for the column instead of matching message text.

**L4 — ignored `enc.write` errors never trigger teardown** · `internal/control/server.go` (many lines)
All server→client writes use `_ = enc.write(...)`. A persistent write failure (client read side gone) produces no early teardown; the server keeps running turns no one receives until the scanner hits EOF.
*Fix:* on a write error in the pump goroutine, cancel `sctx` to tear down promptly.

**L5 — `fork` reports `forked` before `bind` may fail** · `internal/control/server.go:212-222`
`forked` frame is sent before `bind(newID)`; if bind fails, the client saw success but no session is attached (only a generic `fork: bind:` error follows). Recoverable via `switch_session`, but misleading ordering.
*Fix:* bind before emitting `forked`.

## Verified-correct (no change)
- `dispatch.go` parallel core: per-goroutine disjoint writes to `results`, `WaitGroup` join before reads, panic-safe semaphore release, deterministic call-order persistence, cancellation-free result recording (avoids orphaned tool_calls).
- `server.go`: `lineWriter` mutex serializes the two writers; `turnMu` discipline (no lock held across blocking select); single-goroutine ownership of `conv`/`boundID`/`sessionCancel`; scanner copies bytes before send and selects on ctx; no double-close / send-on-closed.
- `sqlite`: all multi-statement writes in a tx under `writeMu` with `defer Rollback` + explicit `Commit`; seq monotonicity serialized; nullable aggregate handled with `sql.NullInt64`; FTS delete-then-reindex avoids dup rows.

## Suggested fix priority
1. **H1 + M1** — both leave `turnDone` open, defeating the drain mechanism the package exists for.
2. **M2** — silent message loss in the outbox.
3. **M3, L2** — robustness/consistency.
4. **L1, L3, L4, L5** — hardening.
