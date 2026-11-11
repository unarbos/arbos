# Phase 3 auxiliary-domain structural audit

Historical audit snapshot: later implementation, corrected native artifact isolation
and current prerequisites are recorded in [the phase report](../structure.md).
The original disk/fork-approval observations below are not current blanket blockers.

Scope: harness, voice, edge, scripts, image, evals. Source inventory/skeleton records 166 files / 36,192 lines in `structure-auxiliary-index.json` (6 harness, 17 voice, 1 edge, 111 scripts, 9 image, 22 evals). All selected source files were indexed for definitions/imports/special-case locations; detailed bodies were inspected at the seams below. Historical scoreboards, generated media, dependencies, secrets and runtime state were excluded. This is a targeted whole-domain audit, not a claim that every statement was individually reviewed. No source edits, branch changes, production calls, deployments, or test removals.

## Redesign 1: one pure cancellation-result presentation module

Recommended and useful, provided the shape and helpers move together and generated-input laws exercise the new seam. Place `StopJobResult`, `sweepClause`, and `stopJobText` in a colocated pure module; index.ts should import its renderer and result type directly. Avoid a generic renderer interface, adapters, registration abstraction or Effect conversion for this synchronous formatting.

Problems it resolves:

- Needless link: `harness/arbos-pi-ext/index.ts:111–181` owns pure cancellation evidence rendering in the same module as import-time environment configuration (`:24–30`), filesystem audit/offload behavior, runtime TypeBox schemas, and pi tool registration (`:770–785`). A consumer of the pure behavior must load a runtime module and inherit its startup conventions.
- Hidden link: the structural type, named/unnamed process sweep wording, and result rendering describe one cancellation evidence contract. They should evolve together in one module. The registered tool only needs to request cancellation and render the result.
- Concrete consumer: Phase 8 can import the pure module for laws without configuring an agentd endpoint, filesystem audit locations, session roles, or a fake pi. This is a concrete testability/dependency improvement; moving the function only to shorten index.ts would be churn.

Behavior that must stay: absent process evidence must not become positive/negative invented evidence; a job state and a process sweep are distinct; failed-to-read session IDs and unconfirmed causes reach the text; notes and all existing visible wording remain unchanged. Existing registered-tool/real-loopback E2E tests are retained.

## Redesign 2: explicit voice call ownership of async lifetime (blocked)

Potential shared fix: one declared call lifecycle and explicit ownership of long-lived versus per-utterance tasks, with awaited shutdown at ownership edges. Do not add a generic task framework without first modeling these actual lifetimes.

Problems it would resolve:

- Hidden convention: `voice/arbos_voice/worker.py:283–294` keeps `_tasks` and `_pending_tools`, `CallUpdates` separately owns `_task`/`_pending` (`updates.py:119–134`), `EventBatcher` owns `_task`/`_closed` (`relay.py:142–173`), `ParticipantListener` owns `_task`/`_pending` (`listener.py:55–73`), and `Vision` owns `_task`/`_closed` plus per-camera tasks (`vision.py:136–163`, `:205–231`). Shutdown does not uniformly await all cancelled children, and not every start/stop resets the same fields. Callers must know each object's restart and quiescence conventions. This is an observed structural inconsistency, not a demonstrated claim of a current user-visible bug.
- Multiple modes/flags in `VoiceCall` couple audio routing, linked-human selection, speaking/thinking state, pending work, and departure. This belongs in Phase 4's state-machine inventory; it should not be rewritten through unrelated per-method cleanup.

Concrete blockers: the baseline Python environment cannot collect the voice runtime tests (`pytest_asyncio`, `aiohttp`, and LiveKit stack unavailable), and the task forbids live service verification. Audio interruption/turn-taking and the charged-call stop path need pinned offline behavioral tests before this cross-module redesign. `usage_loop` behavior is money-related: preserve unlimited accounts, relay cost stops, and the distinct leak guard. Do not alter it silently.

## Cross-language contract that needs parity verification

`edge/sites-worker/src/index.js:21–36, :139–177` and `crates/arbos-core/src/sites.rs:35, :130, :425` intentionally implement compatible reserved names, routes and path safety in different runtimes. A shared language-neutral route/path fixture, consumed by both native and Node tests, would make required cochanges visible. Do not try to make a Cloudflare Worker import Rust internals or merge independent deployment code. Native verification is currently blocked by host build headroom; retain existing behavior and report this residual contract gap. One `new URL(request.url)` per edge request is necessary to route its host/path and is not gratuitous parsing.

## Intentional exceptions and things left alone

- Deny rules in `harness/arbos-pi-ext/deny.ts`, `image/arbos-shell`, and `image/hooks/claude/pre-tool-use.sh` sit at separate execution boundaries. Do not replace independent checks with a single optional caller convention, weaken fail-closed behavior, or deduplicate their authorization decisions merely because patterns resemble each other. A future shared fixture can check intended parity without removing enforcement.
- Prime skill entrypoints are deployment/runtime adapters: atomic file handoff for ask_user, authenticated loopback model proxy for cheap_model, and project MCP JSON-RPC for delivery. They use different protocols; there is no common HTTP wrapper worth introducing across all three. Their small size alone is not surplus.
- `voice/arbos_voice/pricing.py` maps distinct external model IDs. Currently equal prices do not imply that two models must change together later. Keep the named values independent; billing changes require behavior-pinning tests and owner review.
- Voice `Relay` already concentrates HTTP error handling, authorization headers and timeout policies behind one interface. Endpoint-specific helpers carry domain meaning and are not useless pass-through wrappers. Best-effort event reporting intentionally cannot stall the audio path.
- `scripts/release/mac-paths.sh` is an existing single source consumed by release decisions and notes. `release-gate.py` deliberately calls the real Mac change predicate and reads the iOS workflow rather than cloning them. Preserve those seams.
- The large feedback/deploy/merge scripts combine effectful orchestration, approval gates and artifact verification. File length alone does not justify a broad rewrite. Their owner approval, canary hold, release identity and fail-closed decisions are correctness constraints. No production execution or release behavior was changed.
- Eval runners deliberately select their isolated state directory before importing the long-task relay helpers (`evals/convergence/ab.py:30–42`; `evals/long-tasks/lib/relay.py:13–18`). This import-order contract is a maintenance constraint. A passed context would be appropriate if these CLIs become reusable libraries; adding that abstraction now would touch many production-facing eval helpers without a current consumer. These programs default to a real relay, so they were not executed.
- Sleeps in voice quiet-window detection, approval polling, operational backoff and end-to-end evaluators encode physical/service timing or an explicit retry policy. Do not delete them as generic performance cleanup. Preserve calibration and measure before changing cadence.
- Local baseline shell failures are concrete platform constraints: Bash 3.2 rejects a command-substitution/case construct at merge/selftest.sh:350; rebase/verify-cache helpers assume GNU stat. No tests were removed to hide them. These scripts must be verified with their supported Linux/GNU toolchain or receive explicit portable-utility regression coverage.

## Comments and tests reviewed last

No complete test was proven worthless, so none is recommended for deletion. Existing stop-job E2E tests guard the exact product output where fields previously vanished. The absent-field negative control deliberately passes both the old and fixed renderers but catches a different regression (inventing evidence); it is not worthless. Tool-prefix byte identity is an externally material cache contract, not cosmetic snapshot coupling. Deny/kill tests include both positive and negative controls. Merge/release selftests contain deliberate mutants and failure-path controls; their source-level assertions should not be discarded as implementation mirroring without testing their real effect.

Long comments that explain external bugs, proxy/toolchain limitations, billing rules, canary behavior or why a negative control exists are independently useful constraints. Section labels or narration of an adjacent expression can be removed only in the touched extraction; do not erase external rationale in a whole-tree comment sweep. Voice tests use elapsed sleeps and could be made deterministic only alongside a clock seam and preserved audio rules; no speculative rewrite was made.

## Exit opinion

The small cancellation-renderer extraction is worthwhile and locally verifiable with the existing 14 harness tests plus direct pure behavior checks. The auxiliary domain still has the explicitly described voice lifetime and cross-language contract work; do not report a whole-codebase structural exit as passed while those remain unverified. Preserve them as blocked work with the actual prerequisites above. No independent remaining small refactor was established strongly enough to justify source churn in the reviewed adapters.
