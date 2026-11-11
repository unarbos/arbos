# Forwarded attachment allocation regression

A forwarded Copy now passes the received `Bytes` directly into a reusable reqwest body. It avoids materializing another whole `Vec`. Sticker and voice buffers move when their `Arc` is unique; shared buffers retain the previous clone behavior. Sending, chart fallback order, Path reads, per-chat FIFO, errors and refresh/replay policy stay serial.

Run the retained actual-source regression on macOS with the locked Rust dependencies already cached:

```sh
python3 .improve/perf/uploads/check.py --source-root "$PWD" --work /tmp/arbos-upload-check-new
```

The work directory must be fresh. The runner creates an isolated locked HTTP-only Rust leaf, uses a unique Cargo target, builds offline with network denied, then permits loopback only for generated local HTTP/files. It extracts the actual upload/fetch/send tranche and includes the real classifier and ChatSendQueue. DTO/account collaborators are local fakes; this is not GPUI/LiveKit/CRDT runtime coverage. The fixture matches either the standalone one-argument or composed owner-aware refresh signature without changing extracted production bodies. Auth/account collaborators are fakes: owner refusal here is a collaborator control, while production ownership is verified in Phase4. SDK type/Send compatibility is separately recorded in SDK-CLIPPY.json.

The 18 checks comprise 11 functional/queue cases, three same-origin307 byte/length replay cases (Path, Copy, Sticker), three serial-pipeline controls and one allocation regression. The allocation check fails on the unchanged source (two payload-sized allocations) and passes after (one). Shared and unique Sticker bytes and voice bytes are checked, along with missing files/refs, input error order, one PUT401/no retry, GET refresh-once/original-owner refusal, metadata/chart links, pathname replacement and queue clear/FIFO. Source hashes must remain unchanged during the run.

## Measured result

See [results.html](results.html), [BENCHMARK.json](BENCHMARK.json) and [ALLOCATION.json](ALLOCATION.json). Every pair matches full-body hash, length, MIME and ordered output receipts; both implementations remain serial. Two separately measured datasets retain all cases:

| Copy workload | Before median ms | After median ms | Paired saving ms, 95% CI |
| --- | ---: | ---: | --- |
| 1 × 64 KiB | 31.526 | 31.551 | −0.013 [−0.112, 0.028] |
| 4 × 64 KiB | 125.350 | 125.402 | −0.080 [−0.883, 0.982] |
| 1 × 4 MiB | 44.881 | 44.523 | 0.325 [−0.073, 0.500] |
| 1 × 16 MiB | 85.584 | 83.660 | 2.139 [1.281, 3.533] |

These are 21 randomized pairs per case, Rust dev opt0/debug0, fixed12ms per loopback GET/PUT, with local CPU gates held. Only the16MiB latency result supports a gain (paired sign-test p≈0.000021). This does not establish app, device, Internet or production latency.

A separate21-pair allocation-counter run on a shared host measures cumulative requested allocation/reallocation sizes, including local HTTP collaborators; it makes no latency claim:

| Payload | Requested bytes before → after (medians) | Paired byte saving, 95% CI | Payload-sized allocation events |
| --- | ---: | --- | --- |
| 4 MiB | 15,313,432 → 11,119,119 | 4,194,313 [4,194,313, 4,194,313] | 2 → 1 in every pair |
| 16 MiB | 53,017,257 → 36,240,032 | 16,777,225 [16,777,225, 16,777,225] | 2 → 1 in every pair |

Peak incremental Rust heap remains essentially unchanged (about8.47/33.59MB), because response accumulation sets the maximum. These counters are not RSS or a direct measurement of physical bytes copied. Source GETs can be unbounded; the existing serial scheduling and its working-set ceiling are retained.

## Withheld experiments and caller trace

Two experiments are intentionally absent from the product patch. Streaming an opened Path file reduced one exploratory heap maximum but broke existing307 replay and was slower in an unoptimized large-file sample. No manual redirect/auth-retry mechanism was added. Two-job batching was also removed: actual SendMedia supplies one picker item; regular sends supply Path-only batches; forwards supply Copy-only batches. Once extra materialized buffers are constrained, constructed multi-owned-sticker/mixed batches do not demonstrate a reachable current throughput gain. Path/Copy/chart-heavy sends remain serial.

The producer evidence is `sync/relay.rs` Send/SendMedia/Forward/ForwardMessage, `app.rs::send_media`, and `ui/composer/fun.rs::pick_sticker`/sticker cells. Bundled/fallback picker bytes can become unique after the popup releases its click closure, while relay-cached bytes can stay shared. The unique-buffer fast path is conditional; it is not a demonstrated universal picker speedup. Whole-app slowness remains outside this microprofile.

Before/after relay hashes are a8c95f914e9036e5ff8f4cfb4a2f694cfd2665497769c453b276e8ce0d66dc7a /973433f5b9eb4e0731180ae6a818130d44882dd0d5508fed13035b38fb419dbc. The preimage overlays only the three prior hydration helper bodies on the final native SDK base. Full actual-SDK all-target Clippy passed after all12 workspace packages were forcibly rebuilt in an isolated target. Independent source review is [REVIEW.md](REVIEW.md).

The retained regression runner rebuilds actual source. The paired benchmark and allocation JSON retain the measurements and instrumentation in driver.rs; a portable paired-timing orchestrator is not part of this directory. The recorded SDK gate is the composed Phase3+4+6 source, not a standalone Phase6 SDK claim.

Standalone Phase6 source is17f6e0d0f54ab902881e37f56ab7d59d8925d44dfdf738be30e105be0020590b. Its18 actual-source checks pass in19.300s with locked/offline build and unchanged source (STANDALONE-GATES.json). SOURCE-COMPOSITION.json records the narrow authority-preserving resolution; both original upload bodies were byte-equal across the standalone and measured compositions.
