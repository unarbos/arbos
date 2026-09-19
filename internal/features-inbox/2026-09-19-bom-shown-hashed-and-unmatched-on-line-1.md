---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# A UTF-8 byte-order mark is shown as line 1's text, hashed into its anchor, and hides line 1 from an anchored grep

Found by the features agent's own probe (a machine we did not choose: a
Windows-authored repository), 2026-09-19, on `main` `1ac1526b`. Fixed in the
same cycle: [#748](https://github.com/unarbos/arbos/pull/748).

## What happened

A PowerShell script saved with a BOM (`\xEF\xBB\xBF` then `param($x)`), as
PowerShell ISE, Notepad and many .NET tools save files:

- `read` showed line 1 as `1:cme|\u{feff}param($x)` — an invisible character
  in front of the text, for the model to copy into its next edit — and the
  anchor `cme` was the hash of the text *with* the mark.
- The hash of the text as the model sees it (`param($x)`) did not resolve:
  `anchor 1:zgh not found. line 1 is now 1:fza|\u{feff}param($x, $y)`.
- `grep "^param"` answered 0 hits: the walk matched line 1 with the mark in
  front, and the index path the same.
- The classic `old_string` edit worked (a substring search), and the mark
  was kept.

## What holds now

The mark is the file's, not line 1's. Both `read` paths (whole and streamed)
show and hash line 1 without it and put one line above the file —
`[the file starts with a UTF-8 byte-order mark; it is not shown here and
every edit keeps it]`; both `grep` paths match line 1 without it; the
hashline `edit` strips it for resolving and puts it back in front of
whatever line 1 becomes. A file without a mark is unchanged in every way.

Unit test `fs::bom_tests` in the probe's shape.
