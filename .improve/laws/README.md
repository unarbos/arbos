# Generated laws over the actual pure Rust modules

This independent dev-only package runs TypeScript property checks against `crates/arbos-core/src/redact.rs` and `crates/arbos-mac/src/ui/urlwrap.rs`. The Rust bridge imports those actual source files. TypeScript generates inputs and checks returned values; the native process returns no pass flags. No application services, secrets, databases, network providers or native UI runtime are involved.

```sh
pnpm --dir .improve/laws install --frozen-lockfile --ignore-scripts
pnpm --dir .improve/laws build:native
pnpm --dir .improve/laws typecheck
pnpm --dir .improve/laws test
pnpm --dir .improve/laws record
```

Use `--offline` for pnpm once dependencies are cached. The native build is locked/offline and needs cached root-lock Rust dependencies; see [native/README.md](native/README.md). Node 22 runs TypeScript with native type stripping. Paths resolve relative to the source modules, so the suite also works in an isolated checkout without hardcoded user/worktree paths. `record` writes `RESULTS.json`; `GATES.json` retains verification commands, durations and source hashes.

Effect 4.1.2 exposes Schema-derived generation, shrinking and replay through `effect/Arbitrary`; it has no `effect/FastCheck` export. This uses the installed public replacement (`schema`, `checkEffect`, `formatCheckFailure`) rather than adding another testing library. Exact dev-only dependencies are Effect 4.1.2, TypeScript 5.9.3 and @types/node 22.20.5. None is added to product manifests. Installation used frozen lock/ignore-scripts; only the missing2,438,360-byte pinned typings package was downloaded after inspecting official registry metadata, followed by a passing frozen/offline reinstall.

| Required law | Seed | Generated domain and assertion |
| --- | ---: | --- |
| Redaction idempotence | 81001 | Generated Unicode context, tokens from17 families, synthetic bearer/env/placeholder text; each public redactor is unchanged by applying it twice. |
| Confident token removal | 81002 | All17 families per case; assert each exact token is confidently recognized before requiring removal by patterns/scrub. Valid family-specific alphanumeric, mixed `-`/`_`, all-hyphen and all-underscore body variants. |
| UTF8 key byte ranges | 81003 | Nonempty confident finds in generated Unicode context; ranges ordered/nonoverlapping, inside UTF8 scalar boundaries, and slices equal returned values. |
| JSON scrub compositionality | 81004 | Bounded nested JSON template with generated quoted/nonLatin keys, string values and numeric length; output keys/array positions/nonstring scalars unchanged, every string leaf exactly equals actual standalone `scrub(originalLeaf)`. |
| URL visible-text round trip | 81005 | Arbitrary Unicode prefix/suffix, a fixed Unicode host/path URL with generated query body, a second ASCII URL, fixed surrounding punctuation; `plain(hint(text)) === text`, with real HOLD/BREAK insertion. |
| Confident nonword-ending removal | 81006 | Five source-permitted families at their minimum body length ending in `-`; recognition is required before removal. This exposed a real baseline defect. |
| Nonsecret text preservation | 81007 | Up to128 safe characters (including accent, Greek, emoji, whitespace and punctuation) whose alphabet cannot form a supported prefix/header/placeholder; all three redactors preserve exact text, including empty input. |

Each law runs256 seeded generated cases, size64, max 256 shrink candidates and concurrency 1: 1792 cases total. A separate deterministic test covers all 17 families under all 4 declared body variants. Generated alphabets are representative valid subsets, not an exhaustive enumeration of every body/Unicode/URL grammar. The JSON template has bounded depth, fixed boolean/null values and JSON-wire normalization (including −0→0); it is not arbitrary unbounded JSON. Nonsecret preservation is intentionally restricted to an alphabet with an independent no-secret argument. URL law excludes lone surrogates and reserved HOLD U+034F/BREAK U+200B; those are rendering control characters stripped by `plain` by design. It does not claim a full URL parser grammar.

Schema derivation generates all roots. Main/boundary arbitraries also provide one deterministic, Schema-validated semantic simplification because minimum-length arrays of literal choices do not provide useful default character shrinking. Failing checks retain the shrunk input and opaque replay token. To replay a new boundary failure against the **same implementation and Effect version**:

```sh
node .improve/laws/check.ts --boundary 'REPLAY_TOKEN_FROM_FAILURE'
```

Replay tokens are version/implementation-specific. The saved pre-fix token appropriately fails to reproduce after the bug is fixed; it is not an expected green test on corrected code.

## Baseline defect and evidence

The baseline classifier accepted a supported minimum-length token ending in a hyphen, while the redactor's trailing word boundary left it visible. `BOUNDARY_BEFORE.json` records a generated Falsified result before source editing, including one accepted semantic shrink and replay token; `BOUNDARY_REPLAY_BEFORE.json` confirms the same counterexample reproduces. `BEFORE_PROVENANCE.json` records the original native executable hash; the retained local binary was independently rechecked to still exhibit the defect. Binaries remain ignored. `ORDINARY_BEFORE.json` records five earlier passing laws using their initial narrower alphanumeric/ASCII-URL domains; those counts are not presented as identical-domain before/after verification.

The required boundary law now passes after the focused Rust fix, and broad valid-body variants are in the required generator. Native expected-output tests pin minimum/all-hyphen tails, adjacent tokens and historical embedded/overlong matches. The source `KEY_PREFIXES` table stays unchanged. Redacting formerly exposed trailing characters is an intentional security behavior fix, not a claim of byte-identical historical output.

## Transport and verification scope

One persistent native JSONL process serves the generated laws sequentially. The adapter enforces request/response byte bounds, matching IDs, strict Effect Schema response validation, deadlines and terminal failure after malformed output/exit. It drains stderr without retaining unbounded/raw diagnostics and always closes/kills the child. Ten transport checks cover valid Unicode/queued calls, malformed JSON/shape/unknown fields, wrong IDs, exit/start failure, oversized traffic and hangs; these prevent a failed oracle from masquerading as a passing property. Native parser boundary checks are retained in `native/test_protocol.py` (24 checks); its README provides the command.

Final recorded TypeScript gate:18 tests pass/0 fail/0 skip (10 transport,7 laws,1 body-variant test). See GATES.json for exact durations and the final URL-domain strengthening rerun. Run the retained independent mutation controls with `python3 .improve/laws/mutations/run_mutants.py`; [mutations/RESULTS.json](mutations/RESULTS.json) records detection and replay when redaction is bypassed, URL text is dropped, JSON strings are blanked, or all text is blanked; review evidence is linked from the phase report. These finite generative checks and mutation controls are not mathematical proofs.

Bend2 (`bend2` and `bend`) is unavailable. Formal proofs remain under “Needs the owner”; no toolchain install or proof claim is made. Money/signing/replay-protection, native room lifecycles, approvals beyond the independently tested Phase 4 seam, durability, concurrency and whole-app behavior still require their real native/integration gates. This package verifies the stated pure-module laws only.
