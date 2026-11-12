# Reproduce the reviewed combined source projection

This v3 package uses immutable final Phase3 `658266a56e21eff1f740b6040d802033b4620873` and Phase4 `03a12e30e7d956070cfcacfd0e30cd35fca16598`. The other five code-input commits remain pinned in pins.json. The assembler itself remains byte-identical `80f96bc0928bb14fd95820a739e1f69a2ca412b1a7bd2b875ace88bddf3eb987`; v1/v2 recipes and execution evidence remain separate archives. This package contains no copied or relabeled v2 execution receipt.

The input changes are exact reviewed data: the iOS carried guard now covers AppModel, ChatView, Transcript, LibraryList and the intentional source-consumer checker/README extension. Together with five existing Mac paths and CI this gives twelve explicit P3-to-P4 source/diff guards. Common test/proof files carry only when complete blob and mode are identical. AppModel's uploads composition now requires final P4 cc2932a3 and produces f0f596d4 using the unchanged5851-byte reviewed patch. Main-only P6 AppModel is never substituted. API79a41459, relay781cdd41, people39855dcf, canonical redaction19199136 and CI8a2faa25 remain the exact reviewed outputs. Unknown conflicts or mismatches refuse.

The new Swift workflow9863a44d is a normal Phase3 change and exact Phase4 duplicate. Later pinned phase diffs do not touch that workflow; no general workflow exception was added. Its expected final hash is recorded for the post-assembly manifest check. All eleven semantic guard/patch inputs and assembler bytes are checked at open and close by the existing recipe.

Only scoped prerequisite checks are recorded here: twelve standard-library negative contracts passed, twelve exact carried source/diff guards passed, and six independent one-file compositions passed. These are not a combined execution. The assembler writes its actual tree ID, file count, bytes, source manifest and provenance when explicitly run; no final run count/tree is prefilled in this README.

From the checkout root, after reviewing pins and inputs, use a fresh private output:

```sh
python3 .improve/evidence/finish-validation/assembler-final/candidate-v3/handoff/assemble.py --repo . --pins .improve/evidence/finish-validation/assembler-final/candidate-v3/handoff/pins.json --output .improve/evidence/finish-validation/assembler-final/final-003
```

The output must not exist and must remain under this checkout's private `.improve/evidence/finish-validation`. The tool never fetches, commits, merges, updates refs, pushes, deploys or executes application code. It creates immutable Git objects and confines mutable index/materialization to its own fresh output. It refuses unsafe tracked paths, `.env*`, unsupported modes, missing/floating pins, unknown source conflicts, changed assets/lockfiles, changed reviewed outputs, concurrent input/ref movement and live HEAD/index changes.

EXPECTED-PROJECTION.json lists fifty exact anticipated C19-to-v3 changes, each with pre/post mode and SHA256, including all nine corrective iOS Swift files. After the actual assembly, the entire observed changed-file list must equal that inventory and every other path must match419d exactly. No `.improve/**` or extension-wide exclusion is allowed. This preserves Rust/Cargo/build scripts/native fixtures, all Node harness/law inputs and external include_str inputs. Existing receipts remain executions of their original immutable commits; source-compatible reuse is conditional on that final manifest comparison, not a fabricated new CI run. Changed controller/helpers and native Swift inputs have their separate actual verification.

For a new Xcode snapshot, preserve complete compiled source/resources and repository fixture closure. CoreFixturesTests reads `crates/arbos-core/fixtures/**` and `crates/arbos-relay/tests/fixtures/api/**` relative to compiled #filePath. The full assembler includes these tracked assets; a reduced local Xcode source copy must not omit them or soften assertions.

Phase9 pin f1f7e745 is an immutable code-input snapshot containing its historical report metadata. Later finish-PR publication updates the report, recipe and receipts without silently changing these seven code pins. Branch names are movement sentinels during execution, not instructions to substitute later heads. Final publication must distinguish the tested source projection from later documentation commits.
