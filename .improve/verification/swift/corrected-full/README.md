# Complete corrected-source native run

The real, unfiltered Xcode run completed both targets in 1,571.339 seconds. ArbosUITests passed 55 cases, failed none and skipped 13. ArbosTests strictly passed 555 cases, recorded one intentional expected failure, and failed one global lifetime-counter assertion. The process correctly returned 65; this is not a full-suite pass.

The exact inputs are canonical C19 plus the eight reviewed Swift/test overrides. All 2,689 input hashes/modes, unchanged UI test sources/gestures, generated FFI and the all-target loopback-relay plan were checked after the run. The complete repository fixture trees repaired the earlier private source-copy omission. No tests were filtered or retried, and no test timing/threshold was changed.

Every UI scrolling and first-frame measurement completed with rows present. These after-only measurements are verification, with no app-wide speedup inference. The parent-process network policy permits only the reviewed exact Apple testmanagerd socket; this is not universal simulator egress isolation.

A separate DEBUG observer/test-only successor is being prepared. Its later full-unit and five-case Avatar evidence must remain distinct from this completed predecessor full-UI/performance run.
