# Calendar contract correction

`calendar.patch` changes three existing files. `LibraryList.dayLabel` and `stamp` now set the formatter timezone to their injected calendar timezone (two product lines). Locale, date formats, grouping, and default `.current` behavior remain the same. `HomeView` and `BlockedState` product code are unchanged.

The two date methods are byte-identical in the original upstream baseline, P3 and P4 before this fix. Whole LibraryList differs between phases because later media ownership lives elsewhere. PREEXISTING.json records both whole-file identities and exact per-method identities; no whole-file equivalence is claimed.

The new XCTest case covers UTC+9 and UTC−12, weekday/year boundaries and clock words. The existing BlockedState sentence test used UTC timestamps while its public API intentionally formats local time. Its input is now constructed in a fixed Gregorian local calendar; the expected 14:30 sentence and original explicit-UTC formatter checks remain.

The retained Foundation-only runner extracts the actual production methods and actual clock assertions, compiles them with Swift, then runs with a clean environment and network denied. No app, account, service, host clock or timezone is changed. It counts assertions, not XCTest cases:

| Cohort | Passed | Failed | Wall seconds |
|---|---:|---:|---:|
| Original product, new calendar regression and corrected clock fixture |21|8|1.319|
| Fixed product, original BlockedState fixture |28|1|1.235|
| Fixed product and corrected fixture |29|0|1.678|

The final 29 assertions comprise eight original Library words, eight new opposing-zone checks, five BlockedState clock checks and eight comparisons against the exact original methods with `.current`. This is selected-source verification; the complete native XCTest suite remains a separate gate.

From a repository with the patch applied, run:

```sh
python3 -I -B PATH_TO_THIS_PACKET/check.py --tree . --out /tmp/arbos-calendar-new-result
```

The output directory must be new. `baseline-date-methods.swift` contains only the two original method definitions used for default-current parity. The runner validates each exact method SHA256 before compiling. PREEXISTING.json proves those methods are identical in upstream2865, P3d405 and P4624; no MediaOrigin/UI/imports are copied into this fixture. Compiler prerequisites are Xcode Swift and macOS sandbox-exec. Gate JSON contains relative source names and hashes; raw private compiler logs are not included.

This v2 supersedes v1 only by trimming and guarding the baseline snapshot. `calendar.patch` and all29 assertion bodies are unchanged. PORTABLE.json records the one replay of this entrypoint; historical before/after receipts remain unchanged.
