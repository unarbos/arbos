# Cache file lifetime and provenance

The new process-wide StoreLease serializes generated cache-file adoption with connection destruction and unlinking. Old retirement receipts carry the existing WorkTicket and cannot unlink a replacement owner. The short per-file guard spans close/unlink/open, never an account, document or SDK guard. Other cache files continue independently.

The registry retains lightweight exact relay/device and first known user metadata after its Weak SQLite slot expires. Different observed origins or different known users are refused without reading or deleting their predecessor cache. A refused account can continue using its private in-memory state; no automatic import or wipe occurs. Metadata lifetime is the process lifetime and grows with distinct cache paths visited; SQLite connections are not retained by this registry.

Generated ASCII basenames are conservatively lowercased for slot lookup; original paths are used for I/O. Existing parent directories are resolved before lookup. The retained real-file fixture observed this macOS volume to be case-insensitive and proved case aliases/punctuation collisions cannot acquire another observed origin. This covers the generated cache namespace in one process, including different RelaySource instances. It does not claim protection against arbitrary hard links or concurrent hostile directory changes.

## Verification

- Original actual finish_retirement/open_store:12 controls passed,1 failed when paused A destruction was followed by same-file B opening and late A unlink.
- Original actual Shared factory:22 controls passed,1 failed because different known user B reused A cache under the same relay/device path.
- Original Weak-entry module:8 controls passed,1 failed after all A handles ended and a colliding different relay adopted the surviving file.
- Current actual module with generated files:11 tests pass, including same-file blocking until close/unlink completes, other-file independence, late/duplicate receipts, actual case aliases, sequential origin collision and known-user retention. See ../calls/store-owner-gates.json. Current coordinator23-case fresh gate is owned by the independent client agent.

Re-run from any checkout location:

```sh
python3 -I -B .improve/state-machine-checks/calls/check_store_owner.py
```

The checker uses actual store_owner.rs and WorkLifetime plus the exact production file_path/slug/remove_files functions; only the SQLite connection itself is a local file collaborator. It never opens a user cache or makes a network request. Actual SDK/SQLite composition remains a separate full native compiler/test gate.

## Needs the owner: unbound legacy cache recovery

Legacy store.rs uses a punctuation-folded, tail-truncated relay/device basename. Its databases have no persisted exact-origin binding. Consequently neither a filename nor a fresh-process empty owner registry proves which relay/device/user produced an existing file. A missing legacy user identity also cannot establish account provenance. The change above refuses conflicts observed in the current process; it does not solve attribution of old files on first access or unknown identities.

A secure future default would use a lowercase SHA-256 filename over an unambiguous full relay/device tuple and persist the exact origin/account binding. Leave every unbound legacy file unchanged; open a fresh bound cache and offer explicit recovery/import only when the owner can establish its source. This prevents unsafe cache sharing but can temporarily hide unsent pending messages from the previous file. Automatic migration solely by the lossy old filename cannot prove provenance and is unsafe.

The policy choice is whether to accept that temporary legacy-cache/pending visibility change with explicit recovery, or first implement an owner-guided recovery/migration path. No file migration, deletion, SHA-key rollout, startup repair or silent legacy import is included in this patch. This is a concrete data-preservation/privacy policy decision, not a full native build or service-access blocker.
