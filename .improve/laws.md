# Phase 8: make the verified seams checkable

**Question: how do we make this change fully verifiable?** Compile the actual
production functions, generate bounded inputs from explicit domains, assert
observable invariants, preserve reproducible counterexamples and prove the
properties detect deliberate regressions. Then run the adapter and durability
gates at the boundaries those pure laws do not cover. Generated tests are finite
evidence, not a mathematical proof or a substitute for native integration.

The [retained law suite](laws/README.md) uses an isolated dev-only Effect 4.1.2
package. Its current Schema-derived property API is `effect/Arbitrary` with
`schema` and `checkEffect`; that pinned version has no `effect/FastCheck` export.
This is the version-compatible native generator/shrinker, with recorded seeds
and replay diagnostics. No production dependency is added in this phase.
Generated checks exposed and now pin an intentional redaction boundary fix,
described below. Phase 4 separately justifies its runtime Effect dependency at
the actual asynchronous approval trust boundary.

## Laws and coverage

| Law | Domain and observable contract | Coverage / remaining prerequisite |
| --- | --- | --- |
| Scrubbing is idempotent | Applying a pattern/placeholder/composed scrub twice has the same output as once; bounded Unicode text plus synthetic credential/placeholder/header/env shapes | Generated actual-source Rust checks in this PR |
| Non-secret text is preserved | Safe Unicode prose without supported credential/header/env/placeholder shapes is byte-for-byte unchanged | Generated independent preservation check; catches blanket empty-string scrubbing |
| Confident credentials disappear | Each generated synthetic known-shape token must first be recognized confidently, then be absent from scrubbed output; matched ranges are ordered, in bounds and on UTF-8 boundaries | Generated nonvacuous checks; generic opaque guesses are intentionally outside the removal guarantee |
| JSON shape and non-string data survive | Object keys, array lengths, numbers, booleans and null remain identical; each string leaf equals the text scrub of that leaf | Generated bounded nested JSON through actual `scrub_value`; object ordering is not a semantic guarantee |
| URL hints preserve visible/copy text | `plain(hint_bare_urls(text)) == text`; URL break hints must not drop visible characters | Generated scalar Unicode around HTTP/HTTPS links and punctuation. Excludes pre-existing reserved HOLD/BREAK characters and lone UTF-16 surrogates. Does not test font geometry/GPUI |
| A terminal approval refusal cannot be reopened | Later checked/polled/park-answer events cannot turn denied, parked, failed or blocked-pending outcomes into permission | Phase 4 owns 256 Schema-generated cases over its new reducer, plus real-hook regressions. Its module does not exist on this independent baseline branch; no copied mock or conditional skip pretends otherwise |
| Room position agrees with full state | Missing room gives `(0,0)`; append/checkpoint updates agree on head/floor; SQL errors remain errors | Phase 7 SQL equivalence and written native regressions. Arbitrary native interleavings require the unavailable full relay build |
| Retries and durable progress are safe | Duplicate batches do not append twice; a committed cursor never overtakes successfully applied updates; checkpoint import and cache position agree | Existing protocol tests and Phase 3/4 findings. Failure-injected native/sidecar adapter laws remain blocked by native gates; no claim that known durability findings are fixed |
| Account/session ownership survives callbacks | A completion from an old identity or canceled call cannot republish state or clear another room's pending sends | High native lifecycle findings; need runnable Mac/iOS/Matrix adapter regressions before transition rewrites |
| Credit balances and charge caps obey the ledger | Debits and refunds reconcile with balances; configured caps are never bypassed; failed multi-step writes do not lose money | Billing/auth is high risk. No financial behavior changed; production-version ledger fixtures and native transaction/failure-path tests remain required. This chat application has no owned staking/rebalancing allocation module to translate |
| Local-store migration replay preserves data | Reopening existing schemas is idempotent and adding the worker index preserves complete latest-row selection | Phase 7 fresh/existing-store SQL checks include timestamp ties, NULL and missing workers on bundled SQLite. Native Store open/build still unverified |

The suite links its Cargo library directly to `crates/arbos-core/src/redact.rs`
and its binary to the actual std-only runtime of Mac `ui/urlwrap.rs`. A persistent
JSONL child returns function outputs; TypeScript asserts the laws independently.
The driver does not return a precomputed "pass" value. Existing GPUI tests stay
untouched and are not included in a standalone native test build.

Unicode domains distinguish Rust byte indices from JavaScript UTF-16 indices.
Generated JSON excludes non-JSON values and lone surrogates. URL round trips
exclude the two reserved invisible wrap characters because `plain` deliberately
removes them. These restrictions state the actual contract; they are not
exceptions introduced to hide a regression.

## Intentional redaction fix

A generated property and an independent source review found a supported-token
boundary mismatch: a minimum-length token ending in `-` is classified confidently
but the redactor's trailing `\b` can leave it visible. Before the fix, the
adversarial property falsifies, shrinks to a supported synthetic case and replays
the failure. New actual-source native regressions also fail before the matcher
change. They cover minimum lengths, Unicode/separator tails, adjacent tokens and
legacy false-positive boundaries.

The fix preserves the original word-boundary alternative and additionally accepts
end of input or a captured non-word delimiter, reinserting that delimiter in the
replacement. It keeps the single shape table and replacement markers. Surrounding
text is retained; no lookaround unsupported by regex-lite is introduced. The
security behavior intentionally broadens removal for already-supported shapes.
This source region overlaps Phase 6's cache optimization; the finish report must
record the tested combined resolution rather than imply conflict-free merging.

## Verification and limits

Seven laws pass 256 generated cases each (1,792 total), seeds 81001–81007. The
18-test TypeScript gate passes with zero failures/skips: ten transport checks,
seven laws and one deterministic 17-family × four-body-variant check. The final
Unicode URL strengthening reruns eight law tests successfully; raw results and
source hashes are retained. Four independently mutated actual functions are
detected and replay their counterexamples, each with a passing 256-case control.
The [independent review](reviews/laws.md) explains scope and negative controls.

| Gate | Before | After |
| --- | --- | --- |
| Workspace formatting | Phase 0 pass 3.016 s | pass 2.448 s |
| Actual-source leaf fmt/check/clippy | pass .055/1.915/1.721 s | pass .051/.808/.439 s |
| Actual-source native unit tests | original 9 pass, 1.142 s; new regressions intentionally red: 10 pass/2 fail, .924 s | 12 pass/0 fail/ignored, .529 s |
| Leaf release build | pass 3.363 s | pass .861 s |
| Native JSONL protocol checks | new adapter: initial 24 pass | retained 24 checks pass; see native protocol record |
| Strict TS typecheck | new isolated test package | pass .690 s |
| TypeScript tests | five initial narrower laws pass; not the final domain | 18 pass/0 fail/skipped, 2.150 s; final Unicode law rerun 8 pass, 1.692 s |
| Generated adversarial boundary law | Falsified after 1 case, 1 shrink; failure replayed before fix | all 7 laws × 256 pass, 1.529 s final record |
| Four independent actual-function mutations | unmodified controls each pass 256 cases | all mutations falsified and replayed; portable runner retained |
| Full workspace/native integration | blocked/unmeasured | blocked/unmeasured |

The before/after gate times include cache differences and are not speedup claims.
The security correction intentionally also removes some formerly exposed trailing
hyphens from longer supported tokens; historical byte identity is not claimed.
Full workspace clippy/check/test/build and device/CRDT/network integration remain
blocked. Original pure-core tests can run in the small actual-source leaf crate;
that does not make the full native workspace green.

## Needs the owner

Neither `bend` nor `bend2` is installed. Per the requested skill, this does not
block retaining runnable properties. Recommended default: keep these checks and
agree on a pinned Bend2 toolchain and a small proof target (for example terminal
approval absorption) before claiming a proof. No unexecuted proof file is offered
as established verification. Native law coverage also depends on the existing
build-capacity/fork-CI decision recorded in the final report. Also review the
intentional supported-token security fix: recommended default is to retain the
failing-before regressions and removal behavior, preserving delimiter text.
