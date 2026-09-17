# qal-j25: the desktop rig imported a driver five hours older than the app it drove, from the store rather than the app's own commit — so a check meaning "the settings surface is on screen" keeps passing on a field whose meaning changed

- Measured 2026-09-17 22:35 on `qa-vm2`. Store copy `internal/parity/arbosdriver.py`, mtime **07:06**; the app's own copy `desktop/driver/arbosdriver.py` at `origin/main`, last changed **12:32:58** (`4feabfd0`), 15 lines apart. Found by reading the inbox rather than by a failure: `internal/features-inbox/2026-09-17-settings-is-a-tab-now-strip-and-rig-changes.md`, from the settings-surface worker, which says plainly where its change lands in this rig.
- Class: a rig fault that cannot fail loudly. A driver older than its app reads the fields it knows and misses the ones it does not; nothing raises.
- Feature: `deploy/cycle.sh` step 3b2, `ARBOS_QA_DRIVER_DIR`, and `deploy/vm-loop.sh`'s default, which preferred the store's copy whenever it existed.

## What the difference was

Settings stopped being a window and became a tab. Per the note, and confirmed against both copies:

| the app now | the store's 07:06 driver | the app's 12:32 driver |
|---|---|---|
| `state()` carries `front` (`"project"`/`"settings"`) | absent | present |
| `state()` carries `settings_section` | absent | present |
| no `"settings"` window kind; `use_window("settings")` fails by design | still references it | one reference |

And the trap the note names in its own words: **`settings_open` survives with a changed meaning** — the
tab is open, not a window — and "that is the one way an old assertion can quietly keep passing". A rig
reading `settings_open` to mean "the settings surface is on screen" now agrees with the app on the field
and disagrees on the fact. The stale driver cannot even see `front`, which is the field that would settle
it.

The store's `internal/parity/ui_pass.py` is worse: **11** references to the removed settings-window
machinery (`use_window("settings")`, `close_second_messages`, `wmctrl -c Settings`) against 1 in the
repo's. That file is the parity loop's, not this loop's, and is left to its owner — but it is the same
staleness in the same directory.

## Why this rig read the wrong one

`vm-loop.sh` sets `ARBOS_QA_DRIVER_DIR` to the store's `internal/parity` **whenever that file exists**,
falling back to the repo only when it does not. So the store copy always won. The store copy is the
parity loop's working area; it is a reasonable place for that loop to keep its rig and the wrong thing
for this one to import.

## The fix

The driver is versioned with the app it drives, so `cycle.sh`'s desktop step now takes it from
`$wt/desktop/driver` — the same commit as the binary it just built — copies it to local disk (never
imported off the FUSE mount, `qal-j23`), and says which commit it came from:

```
-- desktop driver: 1 file(s) from …/repo-track/desktop-main/desktop/driver (app's own commit: b1c8e82a62b1)
```

The store's parity directory is used only when a worktree has no driver, and a branch with neither
raises `!! NO DESKTOP DRIVER`.

Verified at `80e6994280f8` with the app's own driver (`eda350c8`): `desktop-rapid-session-switch` and
`desktop-fresh-place-no-notice` both pass, so the current driver and this rig's selectors agree.

## What it costs backwards

Every desktop result this machine produced before 22:35 was taken with the 07:06 driver, including
cycle 4's desktop leg, which was already running when this was found. Those runs are not wrong so much
as untrustworthy about anything the driver could not see — the settings surface in particular. They
should not be cited for a settings claim.

## The family

Third instance today of one file with two homes where the stale one wins: `qal-j19` (a held record in
`runtime/` shadowing the live copy), `qal-j23` (the driver read off the store mount), this one (the driver
older than its app). The mesh worker's remedy for the second-reader script was to delete the convenience
copy and leave the branch as the only source. The general form, worth a sweep: **for every file this
harness reads at run time, name its one home, and prefer the home that is versioned with the thing it
describes.**
