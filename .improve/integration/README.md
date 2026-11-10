# Reviewed redaction overlap resolution

PR #2953 (Phase 6, `373c94b021e70ca0d1ae5049cd0e720b98aa103a`)
and PR #2955 (Phase 8, `e978f28d145d8674cbd5de5b1ecb02f3cc2f5d2d`)
independently start from the same upstream baseline and change the same redaction
region. Their overlap requires deliberate resolution.

[redaction-after-performance.patch](redaction-after-performance.patch) is the
reviewed Phase-8 delta **against the Phase-6 version of this file**. It retains
all three LazyLocks and the concurrency regression; builds the new token matcher
and owned replacement strings once; keeps `KEY_PREFIXES`, the placeholder cache
and classifier cache unchanged; and retains all three new boundary regressions.
The cached redaction loop uses `replacement.as_str()`.

For a local review checkout containing the Phase-6 file, check and apply the
patch, then retain that combined file when resolving Phase 8's overlap. Take the
other Phase-8 suite files normally. The patch is not intended for pristine main,
and an already-resolved or later-modified file should not be overwritten blindly.

```sh
git apply --check .improve/integration/redaction-after-performance.patch
git apply .improve/integration/redaction-after-performance.patch
```

Canonical combined file SHA256:
`1919913639b72ba43ffcff9a51bd29fe3038f08701de5e390463613b2dbde3da`.
The independent review checked patch application and exact resulting bytes.
The blank-line context spaces in this unified patch are preserved deliberately;
its local Git attribute affects patch artifacts only.

The isolated combined validation materializes relevant baseline files and applies
each independent phase patch. Phases 3–7 apply cleanly; redaction is the sole
Phase-8 conflict. The canonical resolution is tested with actual-source core
unit tests and generated laws, together with Phase 6's optimized URL module.
See [the final report](../REPORT.md) and retained verification summaries for counts,
commands, source hashes and remaining integration limits. No branch merge,
production operation or deployment was performed.
