# Phase 8 independent laws and mutation review

Reviewed the actual redaction/URL modules, native source bridge, TypeScript
protocol, generated domains, laws, shrinking/replay and failure-path tests.
The review uses the engineering-review workflow in the local Matt Pocock skill.
Only review evidence and isolated temporary mutations were written by this
reviewer; no product or `.improve/laws` source was edited.

## Findings and disposition

1. **Confirmed baseline secret-boundary defect; fixed with a generated RED.**
   `find_key_like` classified `sk-ant-` plus 19 `a` characters and a final `-`
   confidently, but actual `scrub` returned all 27 bytes unchanged. The redactor
   appended a word boundary to the shared shape, while the classifier anchored
   the complete shape. A minimum-length nonword-ending body therefore could
   not match that redactor. Anthropic, OpenAI and Slack examples were confirmed
   through the unmodified native bridge before the source fix.

   The retained generated boundary failure uses seed 81006, shrinks once to a
   valid canonical Anthropic case, and replays the same counterexample
   (`BOUNDARY_BEFORE.json`, `BOUNDARY_REPLAY_BEFORE.json`). The native owner
   added failing tests before changing the shared matcher. The fix preserves
   the old left boundary and old successful right-boundary alternative, then
   permits a nonword tail or end of input; a consumed delimiter is reinserted.
   No policy change to `KEY_PREFIXES` or classification is used to force green.

2. **JSON law initially permitted blanking all strings; corrected.** Shape and
   one-token absence alone did not preserve ordinary string leaves or positions
   of string-valued array entries. The final compositional law recursively
   compares each string leaf with actual standalone `scrub(originalLeaf)`,
   preserving keys, containers, array positions and scalar values. An isolated
   actual `scrub_value` mutation clearing every string now falsifies the law.

3. **Erasing every text value initially satisfied removal/idempotence laws;
   corrected with preservation.** The required nonsecret law uses a declared
   safe character alphabet and checks exact equality for patterns,
   placeholders and composed scrub. Its alphabet cannot construct supported
   credential prefixes, secret assignments or placeholder schemes. A global
   empty-output mutation now fails rather than passing as successful redaction.

4. **Shrinking and range contracts strengthened.** Literal/minimum-length
   Schema inputs originally could offer no useful shrink candidate. Main and
   boundary generators now provide deterministic, Schema-valid canonical
   candidates. UTF-8 range checks use encoded bytes and scalar boundaries,
   compare the returned slice value, and enforce sorted nonoverlapping ranges.

## Matcher review

The right-boundary addition does not admit an ASCII word immediately following
a fixed-width AWS shape, and the original left boundary still excludes embedded
ASCII-word prefixes. Tail capture/reinsertion is necessary to retain delimiters;
tests cover spaces, newlines, quotes, Unicode punctuation and emoji, including
same-family and cross-family adjacent keys. Iterator advancement still observes
the original haystack boundary before the next key. Native tests cover 63
nonword-tail cases and 343 adjacent-pair/separator cases, plus legacy overlong
AWS/embedded-word controls.

The intended fix can mask a whole nonword-ending token where the older matcher
masked a shorter prefix and left trailing hyphens. The old match set is
preserved; exact historical output equality for these newly handled tails is
not claimed. Phase 6's regex cache uses the same matcher construction and needs
the separately reviewed combined resolution; this review does not pretend
independent phase branches apply without that resolution.

The separately retained canonical Phase 6→combined patch was inspected: it
keeps `LazyLock`, initializes the same tail-aware matcher and owned replacement
once, retains classifier/placeholder caches, and uses borrowed replacement
strings during each call. It carries all Phase 6 concurrency and Phase 8
boundary regressions. The native owner reports 13 passing tests plus
fmt/check/clippy on that isolated combined source. No implementation concern
was found in that resolution; it is evidence for the required merge
resolution, not a merged product change.

## Meaning and limits of the generated domains

- Every confident-removal case contains one synthetic token from each of the
  17 supported families. The law requires confident recognition of each token
  before testing removal; absent detection cannot make it vacuously pass.
  Family-permitted alphanumeric, mixed, hyphen and underscore variants are
  exercised, with a deterministic 17×4 coverage test. They are generated
  examples, not exhaustive strings or a second regex implementation.
- Unicode generation uses scalar ranges through U+10FFFF and excludes UTF-16
  surrogates. Explicit multibyte prefixes and UTF-8 Buffer lengths ensure byte
  offsets cannot be mistaken for JavaScript code-unit indices. Reserved HOLD
  U+034F and BREAK U+200B are excluded from URL round-trip inputs because actual
  `plain` intentionally removes them.
- The JSON domain is a bounded nested template with generated keys/string
  leaves, an array and Boolean/null/finite-number leaves. It normalizes -0
  through wire JSON. It is not an arbitrary-depth generator or coverage of all
  JSON shapes. The compositional relation and blank-string mutant make the
  tested structure meaningful within that domain.
- Idempotence cases contain known keys, Bearer headers, secret assignments,
  vault references, secret placeholders and secret environment references.
  They do not rely on mostly random ordinary strings that never exercise a
  replacement.
- URL cases contain real HTTP and HTTPS links, punctuation, a Unicode
  host/path (`https://例.invalid/路径/a/b`) and generated Unicode prefixes/suffixes.
  In addition to `plain(hint(text)) == text`, required insertion
  of both rendering markers rejects an identity hinting implementation. This
  is text conservation, not a GPUI wrapping/geometry or visible-latency test.
- Check options explicitly request 256 runs, bounded shrinking and sequential
  effects. Only `Passed` with all requested runs is green; transport errors,
  exhausted/replay-mismatch results and malformed responses cannot count as
  passing properties. A bounded shrink search is not a proof of a globally
  minimal counterexample.

## Actual-source bridge and isolation

The isolated Cargo library path points to the real `core/redact.rs`; the binary
path module points to real Mac `ui/urlwrap.rs`. Production algorithms are not
reimplemented in TypeScript. Binary tests are disabled so regular source
inclusion uses `cfg(test)` off; existing GPUI tests are preserved. The bridge
returns actual function outputs, not test verdicts or sampled-case lookup data.
Every shrink/replay evaluation makes a fresh native request.

The native reader has bounded allocation/line/depth limits and rejects unknown
fields, invalid IDs and malformed requests without echoing their contents. The
TS bridge validates operation-specific response schemas and IDs, serializes
concurrent callers, drains diagnostics, bounds output and response deadlines,
and rejects future oracle use after transport failure. Missing binaries,
malformed/oversized replies, wrong IDs/shapes, timeout and unexpected exit have
failure-path tests. No local app database, credentials or live service is used.

## Isolated negative controls

`laws-mutants/run_mutants.py` copies actual current source into fresh temporary
packages, applies only the recorded single-function mutation, and builds offline
with the same pinned native dependencies. It imports the **actual** shared law
catalog and bridge. Every target first passes 256 cases against the unmodified
binary, then must falsify against the mutant, then must replay to the same
shrunk input. Product source hashes are checked unchanged after all controls.

| Temporary actual-function mutation | Target law / seed | Observed failure and replay |
| --- | --- | --- |
| `scrub` returns original text | confident-token-removal / 81002 | Falsified on run 1, one shrink; replay Falsified with equal canonical input |
| `hint_bare_urls` drops its first visible scalar | url-visible-text-roundtrip / 81005 | Falsified on run 1, one shrink; replay Falsified with equal canonical input |
| `scrub_value` clears all string leaves | json-scrub-compositionality / 81004 | Falsified on run 1, one shrink; replay Falsified with equal canonical input |
| `redact_patterns` always returns empty text | nonsecret-text-preservation / 81007 | Falsified on run 3, already-minimal string `"1"`, zero further shrinks; replay Falsified with that same input |

The final control's empty-string candidate passes, so a one-character failing
input is already minimal by length. Zero further shrinks is expected there;
the other three controls demonstrate an actual selected shrink path. Raw
results, replay tokens, mutation diffs and sanitized build logs are retained in
`laws-mutants/`. These are deliberate isolated failures, not source regressions
or mutation edits applied to the product.

After the final URL generator added a Unicode host/path, the URL control was
rerun against that exact shared property: 256 unmodified cases Passed,
the source mutation Falsified with one shrink, and replay Falsified with the
same canonical input. `RESULTS-url-drop-visible-character.json` retains that
final rerun; `REVIEW_INPUTS.json` hashes the final reviewed law/bridge/product
inputs. Final range-helper strengthening did not alter the other three target
properties or their recorded shrink paths.

Reproduction after installing the law suite's declared prerequisites and
building its ordinary native bridge:

```sh
python3 .improve/evidence/laws-mutants/run_mutants.py
```

No Bend2 proof was run: the toolchain is absent. Generated tests and mutation
checks are executable evidence, not formal proofs. High-risk auth/call/Matrix
lifecycles, money/ledger invariants, full native integration and rendering
calibration remain outside this pure-law catalog. Their prerequisites and
owner follow-ups must remain in the main report.

Final root gates record 12 native redaction tests, 24 protocol cases and 18
TypeScript test cases, including seven required laws × 256 generated cases
(1,792 generated evaluations), plus deterministic family/variant coverage.
All four isolated negative controls were detected and replayed as detailed
above. The isolated combined Phase 6+8 source has its additional concurrency
test (13 native tests). These are the runnable pure-law/protocol gates; they do
not turn blocked whole-app or formal-proof checks into passing checks.
