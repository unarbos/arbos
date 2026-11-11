# Native artifact isolation correction

A combined gate explicitly expected three append regressions but Cargo returned
zero selected tests from a newer binary built in another same-layout source
snapshot. The count guard failed the verification; exit0 with zero cases was not
accepted. Cargo dep-info uses workspace-relative paths and source mtimes, so one
shared target directory did not prove which snapshot supplied the executable.

Affected shared-target native GREEN records are superseded. Each materialized
source now owns a unique target. Existing immutable dependency artifacts can be
APFS-cloned, but all relevant workspace packages are forcibly cleaned in that
clone before compiling the pinned source. The original/shared cache and all user
worktrees are left alone. Before/after source hashes, clean commands, expected
nonzero counts and fresh results are retained. This is artifact isolation, not a
new compiler trust or performance claim.

The root combined Phase3 recheck now passes35actual native regressions:3append,
11privacy,15sidecar daemon,4mirror,2core route-fixture checks. Four-package
all-target Clippy passes19.673s after forced workspace rebuild. Mac25std-only
checks use direct rustc, Node37checks and guarded voice84cases use different
runtimes, and generated pure laws/independent leaf targets are separate. The SQL
profiler uses a source-locked content-addressed C engine; its paired measurements
are unaffected. Compiler/test wall times are not used as app-speed evidence.
