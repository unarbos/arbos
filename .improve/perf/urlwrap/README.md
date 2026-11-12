# URL hint scanning

This driver includes the actual `crates/arbos-mac/src/ui/urlwrap.rs` runtime
module in a standalone binary with `cfg(test)` off. It needs Python 3 and `rustc`,
but no Cargo dependencies, GPUI, network access or application data. Repository
paths are derived from the script, so it also runs from another working directory.

Check the current code:

```sh
python3 .improve/perf/urlwrap/measure.py
```

Reproduce the comparison (the baseline commit must exist locally):

```sh
python3 .improve/perf/urlwrap/measure.py \
  --baseline-ref 2865cb29b806121d4a90f55ba6ce151a150279a0 \
  --benchmark --output .improve/perf/urlwrap/results.json
```

The 10 exact output goldens cover both earliest mixed-scheme orders, false
`http` prefixes, Unicode, punctuation, absent URLs and incomplete schemes. Each
also checks visible-text round trips; 125 generated combinations check that law
again. Both the original and changed source passed all checks. The existing GPUI
tests in the production module remain unchanged and are not run by this driver.

The measured operation is the public `hint_bare_urls` function, compiled with
`rustc -C opt-level=3`. Each workload receives 8 warmup calls, followed by
`5000 / links` calls per timed sample, with `black_box` inputs and outputs. There
are 31 before/after pairs; execution order alternates each pair. Process startup,
compilation and input generation are outside the timed interval. The raw samples,
source hashes, compiler version and summaries are in [results.json](results.json).

Recorded medians on this Mac (microseconds per call):

| Scheme | Links | Input bytes | Before | After | Speedup |
| --- | ---: | ---: | ---: | ---: | ---: |
| HTTPS | 1 | 36 | 0.233 | 0.161 | 1.44× |
| HTTPS | 100 | 3,690 | 35.169 | 13.368 | 2.63× |
| HTTPS | 1,000 | 37,890 | 2,004.850 | 134.608 | 14.89× |
| HTTP | 1 | 35 | 0.219 | 0.156 | 1.41× |
| HTTP | 100 | 3,590 | 42.984 | 13.131 | 3.27× |
| HTTP | 1,000 | 36,890 | 3,108.083 | 132.083 | 23.53× |
| Mixed | 1 | 36 | 0.228 | 0.157 | 1.45× |
| Mixed | 100 | 3,640 | 17.901 | 13.918 | 1.29× |
| Mixed | 1,000 | 37,390 | 177.275 | 141.250 | 1.26× |

The original finder repeatedly scanned each remaining suffix for an absent
scheme. The changed finder walks `http` candidates once, checking both allowed
prefixes at each candidate. Long messages with many same-scheme URLs expose that
quadratic scan; the 1,000-link inputs are deliberately long stress workloads.

These are isolated formatting measurements, not chat-open, message-send, frame
or end-to-end latency measurements. Whole native builds and GPUI visual tests
remain blocked by local disk capacity and fork CI approval. No wrapping markers,
copy/paste rules, line geometry or physical-device calibration changed.
