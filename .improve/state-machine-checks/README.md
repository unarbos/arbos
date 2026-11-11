# Phase 4 retained checks

This directory retains compact evidence for the implemented approval and voice
controllers. It contains no SDK wheels, credentials, environment snapshots or raw
SDK logs. `GATES.json` records actual RED stages and the final 84/84 guarded SDK
gate (10 unchanged original + 53 worker + 21 vision tests). Different RED stages
used different test sets; their failures are not independent vulnerability counts.

The final local gate imported the actual SDK in a private environment, denied
network in macOS sandboxing, and audited credential/network/subprocess access.
No access attempts occurred. All model, room, audio and camera effects were
synthetic. The routine SDK setup is now available; it is not a remaining blocker.

## Run the source regressions

Follow [the voice test setup](../../voice/tests/README.md) using its explicit four
test files. It declares the test extra and inspected SDK cohort. Do not run the
whole directory: it also contains opt-in live/evaluation scripts.

From the repository root, install the frozen harness dependencies and check the
TypeScript surface:

```sh
pnpm --dir harness install --frozen-lockfile --ignore-scripts
pnpm --dir harness -r typecheck
pnpm --dir harness -r build
pnpm --dir harness -r test
python3 image/tests/test_runtime_packaging.py
```

The final harness has 44 tests: 37 prior approval/harness tests, six generated
voice laws and one protocol test with seven invalid requests. The voice laws use
the actual pure Python modules through an isolated JSONL bridge, with six seeds
84001–84006 and 256 cases each (1,536). The approval terminal law's 256 cases are
separate. These finite checks are not proofs or live-call tests.

## Run independent negative controls

```sh
python3 .improve/state-machine-checks/check_mutants.py
```

This needs Node, Python and the frozen harness install above, but no voice SDK.
It copies the exact pure modules, property test and bridge to disposable paths,
then mutates only those copies. Four source-function mutations must falsify their
target laws; a transport failure cannot count as detection. The stale-completion
mutation falsifies both mute and mode laws. Live source hashes must stay equal.
New results go to ignored `generated/`; retained `MUTANTS.json` includes each
observed one-step shrink and replay record, without temporary host paths.

`CALIBRATION.json` names protected unchanged constants, prompt/speech parameters
and exact unchanged methods. Entire changed controller functions are not claimed
AST-identical. `REVIEW.md` records the independent scoped PASS and its limits.
The phase report retains remaining native and operational High work.
