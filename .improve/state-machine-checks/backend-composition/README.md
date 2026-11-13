# Backend composition checkpoint

Two independent package targets compiled an immutable snapshot of the Phase4 parent plus the five modified sources pinned in SOURCE.json. Each target cloned completed dependency artifacts, then cleaned all12 workspace packages before compiling these sources. Fresh HOME, locked/offline Cargo, validated loopback-only Seatbelt; no live accounts, providers or external endpoints.

Agentd: all-target Clippy with warnings denied, plus46 selected tests (metadata17, boot7, slot4, approval delivery12, parking6). Relay: all-target Clippy with warnings denied, plus54 tests (box rows13, voice24, credits8, accounts9). All100 passed with source hashes unchanged. Package targets were separate; counts are distinct within this checkpoint and overlap earlier component evidence. This is not a full workspace or final combined-branch gate.

The first relay run incorrectly selected voice::tests:: and got zero tests. Its count guard rejected that run; the failed setup is retained in relay-GATES-initial-wrong-filter.json. The resumed run used voice:: and the same unchanged workspace/target, retaining the successful Clippy and box-row checks. No zero-test run is counted as green.
