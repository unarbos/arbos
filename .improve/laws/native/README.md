# Native law bridge

This standalone, development-only Cargo package compiles the actual repository
redaction module as its library and the actual Mac URL module into its binary.
It does not copy either implementation or change product dependencies. The binary
has `test = false`, so the URL module's GPUI visual tests are not compiled by the
portable check; the redaction module's original unit tests run with `--lib`.

From this directory, using the checked-in lockfile and cached dependencies:

```sh
cargo test --locked --offline --lib --jobs 2
cargo check --locked --offline --jobs 2
cargo clippy --locked --offline --jobs 2 -- -D warnings
cargo build --locked --offline --release --jobs 2
python3 test_protocol.py
```

The standard-library Python check runs 24 cases through one actual process,
including byte/depth bounds, invalid requests, UTF-8 ranges, recovery and EOF.
Use `python3 test_protocol.py --binary /path/to/laws-bridge` to check an isolated
integration build.

The executable is `target/release/laws-bridge`. It reads one JSON request per line,
flushes one JSON response per request, and keeps running until EOF. IDs must be
nonnegative safe JavaScript integers. Objects contain exactly these fields:

- `{id, op: "text", text}` returns `patterns`, `patterns_twice`, `placeholders`,
  `placeholders_twice`, `scrub`, `scrub_twice`, and classified `keys`. Each key
  contains `start`, `end` (UTF-8 byte offsets), `value`, `confident`, and
  `suggested_name`. An invalid range yields `value: null` for the caller to detect.
- `{id, op: "json", value}` returns `redact_json` from `scrub_value`.
- `{id, op: "url", text}` returns `hinted`, `plain` (of hinted text), and
  `breakable` (of the input).

The bridge reports outputs, never property verdicts. TypeScript owns expectations,
generation and shrinking. Inputs are synthetic test fixtures, not user data.

Invalid JSON, fields, types or operations produce only
`{id, error: {tag: "InvalidRequest", message: "Invalid request"}}`; the ID is
`null` when no valid ID can be recovered. Parser details and input are not logged.
Each line is limited to 65,536 bytes excluding LF; oversized lines are drained
without retaining them, then rejected with a null ID. JSON values allow at most
32 nested levels. Unknown fields are rejected. The TypeScript transport separately
bounds output, time and process lifetime.

Passing these portable checks does not establish GPUI rendering, full workspace
integration or a formal proof.
