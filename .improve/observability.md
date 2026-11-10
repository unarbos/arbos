# Phase 5: observability

**Status: scoped logging complete; broader observability deferred by the owner.**
The owner will confirm the existing telemetry backend with the maintainer. Keep
these logging fixes and finish the other phases independently. OTLP export,
global definition coverage, a tracing lint, W3C propagation and all-service request
completion events are explicitly deferred, not implemented by this PR. No
collector/account was created and nothing was deployed.

The required [Boris Tane article](https://loggingsucks.com/) was read. Its useful
distinction here is between JSON records and a complete request history: collect
safe business context during the operation, finalize once, and decide retention
after the outcome is known. Error and slow-operation evidence must survive.
Transport instrumentation alone does not supply this context. The existing
admission audit has a separate purpose and must remain authoritative evidence.

## Repository findings

See the complete [backend evidence and implementation prerequisites](reviews/observability-backend.md).
Relay and agentd initialize text formatters and warning rings; relay adds HTTP
`TraceLayer`. Proxy request IDs and prompt-shape hashes already help correlate
model calls. None establishes an OTLP backend or W3C context propagation.
No configured repository OTLP destination was found. External operator settings
were not inspected, so this is not a claim that none exists outside the repo.

Raw syntactic inventory across all 409 Rust files found 2,412 `async fn`
occurrences, 950 pub-visible async declarations and zero direct instrument
attributes. These counts include tests/declarations and are not a production
coverage denominator. Voice has 97 async definitions across 17 Python files,
including tests. A regex inventory is not a passing exported-async lint.

Blanket argument recording would be unsafe: `Principal::Device` derives Debug
and contains a bearer token. Future definition-level instrumentation must skip
arguments and attach explicit safe context after authorization. Existing warning
rings read event fields rather than span context; preserve their account
isolation/filtering when changing the event model.

## Implemented slice

The Prime shell wrapper and Claude PreToolUse writer previously retained raw
command/input previews and interpolated metadata into JSON strings. Quoted or
multiline identifiers could corrupt the record; fresh denied paths could fail to
write an audit event. The source fix emits structured admission events with
identifiers, denial category and an input digest, preserving the original deny
rules and process/exit behavior. No command text or response body belongs in the
new records. Phase 4 separately removes the TypeScript writer's raw preview.

This is one admission event for each recognized nonempty Prime `-c` command and
each Claude PreToolUse call. Prime's other shell passthrough remains unchanged.
An admission record is not a completion event: after `exec`, this wrapper does
not observe the command's final duration/outcome. No completion or tail-sampling
coverage is claimed by this slice.

## Deferred requirements and exact next boundary

| Required result | Current state / prerequisite |
| --- | --- |
| Environment-configured OTLP export | No repository backend contract; owner supplies/chooses existing collector and environment-managed auth. Provider lifecycle must compile and pass loopback-export tests when this deferred integration resumes. |
| Trace every required function definition | Not implemented globally. Instrument native handlers/helpers/request makers at definitions, skip arguments, and preserve pure-core/FFI platform boundaries. |
| Fail lint on untraced async exports and make it pass | Not implemented. Needs AST-backed feature-aware production inventory, genuine negative control, then verified instrumentation. Do not exclude whole domains or substitute an all-green regex check. |
| Authenticated request context on spans | Not implemented globally. Carry authorized user/account/resource context through tasks, blocking work, streams and jobs; preserve ring isolation. |
| Consume/forward W3C traceparent | Not implemented. Use shared request/HTTP seams and allowlisted propagation; existing Arbos routing trace IDs are not W3C IDs. |
| One wide terminal event per request/job/service | Not implemented globally. Streaming response/job owners must finalize once for success/error/cancellation, preserving provider billing and process semantics. |
| Tail retention of all errors/slow work | Not implemented. Requires completed-event and collector buffering/retention contract; head sampling cannot provide it. Retain security admission audit events. |
| Secret-safe fields in every sink | Improved only at the scoped audit writers. Existing upstream error strings, text sinks, voice and other service logs need source-level schemas/redaction tests. |

Earlier native disk/CI limitations were resolved for subsequent phases; they are
not the reason to defer this work. The owner's telemetry/backend decision is the
explicit remaining prerequisite. [Official Rust exporter documentation](https://opentelemetry.io/docs/languages/rust/exporters/)
was checked; a design or dependency declaration is not working OpenTelemetry.
Use loopback collector/provider stubs when the integration resumes.

## Needs the owner

Confirm the operator's OTLP-compatible destination/auth contract, or choose an
optional environment-configured collector on existing infrastructure. Recommended
default: no new account or paid service; keep export disabled until configured.
The owner will ask the maintainer what telemetry is already in use. No action on
this decision is required to finish the other improvement phases. Preserve
anonymous client diagnostics and existing source-redaction boundaries.

## Before / after verification

| Row | Before | After |
| --- | --- | --- |
| Admission/exec controls | 9 passed / 0 failed/skipped; 14.616 s | all retained in final suite |
| Privacy/JSON/fresh-denial regressions | 6 fail (2 assertions, 4 JSON/file errors); 0.217 s | pass |
| Complete audit suite | controls only | 15 passed / 0 failed / 0 skipped; 22.479 s |
| Shell syntax (`bash -n`) | both pass | both pass; 0.003 s each |
| Denial policy | 3 regex rules per script | all 6 rule lines byte-identical |
| Raw input previews in these two writers | present | absent; full SHA256 digest retained |
| Prime admission median (10 per arm) | 22.00 ms | 21.54 ms |
| Claude admission median (10 per arm) | 31.46 ms | 31.23 ms |
| Harness typecheck/build/tests | baseline pass / pass / 14 pass | source unchanged; baseline carried, not rerun here |
| Rust formatting | baseline pass | Rust sources unchanged; baseline carried |
| Full native typecheck/tests/build | initially unmeasured here | Shell-only scope; complete combined native gates are recorded separately in Phase9 |
| OTLP/global trace lint/propagation/wide completion | absent | Deferred by owner; not implemented by audit fix |

Run `python3 -m unittest discover -s image/tests -p test_tool_audit.py -v`.
Tests execute real scripts and benign local commands, with temporary audit paths
and isolated environment, never a service or real credential. They cover deny
precedence/exit codes, argv/exec/PID/signals, stdin passthrough, write failures,
private text absence, metadata round trips, integer timestamps, one event and
fresh denied directories. Privacy assertions run before parsing so invalid old
JSON cannot mask the raw-preview proof. An independent original-writer fixture
with only its clock made numeric isolates metadata escaping from the old macOS
clock-format defect; ordinary metadata parses and quoted/newline metadata fails.

The ten-sample admission timings are noisy process-level smoke measurements, not
statistically significant speedups or application latency results. A baseline
Claude outlier was about 698 ms. No performance benefit is claimed here.
Independent scoped source/test review passes. These local checks do not establish a full Linux/image/native CI verdict.
Later CI/tool availability is recorded in Phase9, separately from this scoped run.
