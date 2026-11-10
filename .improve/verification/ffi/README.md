# Final C19 simulator FFI preparation

The exact final source tree `c19a6e2ab764c53065d8f79e53bbe90f76508c4d` builds for `aarch64-apple-ios-sim` and produces a private one-slice ArbosFFIRust XCFramework. This receipt covers compilation, pinned UniFFI generation, packaging and ABI inspection. It does not claim an app build or XCTest/runtime result.

The source clone contains only the 2,689 regular files in the reviewed manifest, with every hash checked. The completed SDK dependency target was cloned into a new private target. All twelve workspace packages were cleaned for both the host and the explicit simulator target; the seeded old `.a` was asserted absent before the final build. A host-only `cargo clean -p` does not remove cross-target output and was not accepted as final-source proof.

Rust 1.99.1 and existing Xcode 26.2 built the four local FFI dependencies locked/offline in 11.840 seconds. The already reviewed UniFFI 0.32.2 tool generated Swift/header/modulemap in 1.615 seconds; XCFramework packaging took 1.110 seconds. No tools or dependencies were installed. All commands used a deny-network OS fence, fresh HOME/TMPDIR, explicit known public caches and only the new private target. Full source hashes remain unchanged.

Generated Swift (`291724…`), C header (`c25f12…`) and modulemap (`cc6b49…`) are byte-identical to the earlier SDK proof. All 280 declared C ABI exports match the baseline and are present in the actual native FFI objects. Apple's older nm cannot parse newer RustLLVM23 standard-library bitcode in either whole archive; the successful export check therefore inspects this crate's native MachO objects and verifies complete generated-header coverage. It does not claim a whole-archive Rust-symbol audit or behavioral equivalence.

`SOURCE.json`, `GATES.json` and `ABI.json` retain exact source/library/tool/generated/symbol hashes and scoped timings. Paths are relative roles only. Raw logs, user paths, SDK/cache trees and binaries are omitted. The private framework and generated wrapper were handed off for a separate build-for-testing review; no app, simulator, test, signing, provider or cloud operation was launched by this preparation.
