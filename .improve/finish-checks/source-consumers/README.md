# Actual source consumer contracts

The existing macOS Swift CI job runs this small check after selecting Xcode and before building Rust/FFI. It compiles actual production call expressions/callback bodies with inert local collaborators, then executes with a fresh environment and network denied. It needs only Python3, installed Swift and macOS sandbox-exec; no new job, dependency, account, app or device is used.

```sh
python3 .improve/finish-checks/source-consumers/check.py --tree . --out /tmp/arbos-source-consumers-new
```

The output path must be new. Source/fixture hashes and exact assertion counts are emitted. The checks fail if the expected source boundaries move or source changes during execution.

Six checks compile the three actual tail/append/full Transcript.rows calls against a required hiddenSenders argument, then verify the exact nonempty/empty set is passed. This is caller input-contract coverage; the native BlockedHiddenRowsTests still exercise the actual renderer/model. The prior closing-parenthesis string-count assertion is replaced, while every other assertion and all existing native/UI tests remain. Removing one required argument is a retained compile-failing negative control.
