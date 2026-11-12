# Final assembly native-input projection

PASS: final-003 tree `f3903159bb239a21f68a62ec029b82102ffe3f87` matches the frozen final native inputs at all 470 selected tracked paths, in both workspace bytes/modes and immutable Git tree/blob reads. All 192 unique actual Xcode compiler inputs are included, together with project/scheme/resource inputs, source-reading tests, the 110 core/relay golden files, four Rust FFI path-dependency package envelopes, and Cargo resolution/build configuration. The nine reviewed Swift overrides are already part of the tested native baseline; no runtime mismatch was exempted.

Outside this projection, the immutable assembly manifest differs only at 40 proof/report paths and one workflow YAML path. These are reported separately and do not count as native runtime matches. Other domains remain subject to their own verification.

The six frozen generated FFI artifacts and the exact reviewed private SwiftPM lock all match the retained native snapshot. These seven ignored/prepared files are absent from the source-only assembly. Reproducing a build still needs those outputs/package pins or equivalent reviewed regeneration; this comparison does not claim that the bare assembly is build-ready. Generated Swift file mode 644 was checked here against its frozen generation and native snapshot; the earlier historical mode receipt covered only the five framework files.

This task performed read-only file/manifest/Git blob comparison and 11 synthetic validator contract checks. It ran no application, build or Arbos tests and changed no refs, assembly files, simulators or runtimes. The existing final-native full-unit plus five-avatar evidence and predecessor full-UI/performance evidence keep their original distinct scopes.

Private reproduction from the parent evidence directory uses `validate.py` with the frozen BASELINE, final-003 workspace/source-manifest, and `local-xcode-c19/observer-v3-native/source-inputs` as the external prepared-artifact reference. The scripts refuse to overwrite result paths or write inside the assembly.
