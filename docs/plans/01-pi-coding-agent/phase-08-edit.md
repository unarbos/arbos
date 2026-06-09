# Phase 8 — edit tool (exact + fuzzy)

Back to [overview](./overview.md). The crown-jewel tool. Mirror pi
(`coding-agent/src/core/tools/edit.ts`, `edit-diff.ts`) byte-for-byte.

## Goal

Port pi's multi-edit tool with full fidelity, including the fuzzy fallback and
the exact model-facing error strings.

## Status: done

Implemented in `codingspec/editdiff.go` (matcher) and `edit.go` (tool). Exact
then NFKC-fuzzy matching (D13 added `golang.org/x/text`), uniqueness and overlap
rejection, reverse-order application, BOM and line-ending preservation, the exact
success message, and the verbatim error strings. The matcher carries the granted
unit tests (`editdiff_test.go`). `Details` is minimal (`replacedBlocks`,
`firstChangedLine`) per D15; the full display diff is deferred to the TUI. The
schema generator gained nested-struct / struct-slice support for `edits[]`
(D14). Runtime-proven on the real dispatch path: multi-edit, CRLF preservation,
fuzzy match against smart-quote/en-dash content, and the non-unique, not-found,
and overlap error strings.

## pi behavioral spec (faithful reproduction target)

Input: `{ path, edits: [{ oldText, newText }, …] }`. A legacy `{oldText,newText}`
shape and an `edits`-as-JSON-string shape are accepted and normalized first.

Algorithm, in order:

1. Read file. `stripBom` (remember the BOM). `detectLineEnding` (`\r\n` if the
   first CRLF precedes the first LF, else `\n`). `normalizeToLF` the content.
   Normalize each edit's `oldText`/`newText` to LF too.
2. Reject any empty `oldText` before matching.
3. Match each `oldText` against the original normalized content. `fuzzyFindText`:
   try exact `indexOf` first; if not found, normalize BOTH content and `oldText`
   with `normalizeForFuzzyMatch` and `indexOf` in that space.
4. If ANY edit needed fuzzy matching, the whole operation runs in fuzzy-normalized
   content space (`baseContent = normalizeForFuzzyMatch(content)`); otherwise
   `baseContent` is the original.
5. For each edit: must be found (else not-found error); count occurrences in the
   fuzzy-normalized space, must be exactly 1 (else duplicate error).
6. Sort matches by index; reject if any two overlap
   (`prev.index + prev.len > cur.index`).
7. Apply replacements in reverse index order so offsets stay valid.
8. If `baseContent === newContent`, no-change error.
9. `restoreLineEndings` to the detected ending, re-prepend the BOM, write.
10. Result text: `Successfully replaced N block(s) in {path}.` `Details` carries
    a line-numbered display diff and a unified patch (`generateUnifiedPatch`,
    4 lines of context).

`normalizeForFuzzyMatch`, exact steps and order:
- `.normalize("NFKC")`
- split on `\n`, `trimEnd` each line, rejoin (strip trailing whitespace per line)
- smart single quotes `[\u2018\u2019\u201A\u201B]` -> `'`
- smart double quotes `[\u201C\u201D\u201E\u201F]` -> `"`
- dashes `[\u2010\u2011\u2012\u2013\u2014\u2015\u2212]` -> `-`
- special spaces `[\u00A0\u2002-\u200A\u202F\u205F\u3000]` -> ` `

Exact error strings (single-edit / multi-edit variants):
- not found: `Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.` / `Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines.`
- duplicate: `Found {n} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.` / `Found {n} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique.`
- empty: `oldText must not be empty in {path}.` / `edits[{i}].oldText must not be empty in {path}.`
- no change: `No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.` / `No changes made to {path}. The replacements produced identical content.`
- overlap: `edits[{a}] and edits[{b}] overlap in {path}. Merge them into one edit or target disjoint regions.`

Tool description and the four `promptGuidelines` bullets are ported verbatim
(see `edit.ts`).

## Data structures

- `EditArgs{ Path string; Edits []Edit }`, `Edit{ OldText, NewText string }`.
- Result uses Phase 1 `Details` for the diff and patch.

## Dependencies

Phases 1 and 4. Pairs with Phase 7 for shared line-ending helpers.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness applies single and multi-edit calls and asserts file content and
the returned diff.

Unit (granted exception). A case table for the pure matcher reproducing the spec:
exact, fuzzy (NFKC, each smart-quote class, each dash, NBSP and the other special
spaces, trailing whitespace), not-found, duplicate, overlap, empty, no-change,
CRLF and BOM preservation, reverse-order application correctness.
