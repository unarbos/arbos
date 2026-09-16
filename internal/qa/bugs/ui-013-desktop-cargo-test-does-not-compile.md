# ui-013: `cargo test` in `desktop/` does not compile on `rust` (and the integration head)

- Feature: desktop crate tests (`desktop/src/view/component/transcript.rs`)
- Severity: low for users, high for the loop: no desktop unit test can run, so CI (if it runs them) is red or they are skipped.
- Found while adding the qa-030 regression test (PR [#120](https://github.com/unarbos/arbos/pull/120)).

## Repro

`cd desktop && cargo test --lib` on `rust` @ `c066c37` or `cursor/release-integration-52cd`.

## Actual

```
error[E0425]: cannot find value `WORK_ROWS` in this scope   --> src/view/component/transcript.rs:3425
error[E0425]: cannot find function `work_bits` in this scope --> src/view/component/transcript.rs:3421
error[E0425]: cannot find function `work_bit_index` in this scope
```

`long_runs_fold_to_the_last_rows_until_opened` tests a folding helper that no longer exists (the work-rows folding was reworked; the test was left behind).

## Suggested fix

Delete the test or rewrite it against the current folding code. Whoever owns the transcript component. #120 had to disable it locally to run the kernel tests.
