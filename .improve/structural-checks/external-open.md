# Native file handoff ownership

The relay and fixture transports no longer import `ui::attachments` merely to
open a fetched file. The existing `reveal` and `open_with_system` implementations
and their external platform requirements move unchanged into `external_open.rs`;
application/fixture/relay callers share that std-only owner. Attachment rendering,
Quick Look/default-app selection, OS flags, inherited/null stdio and best-effort
error logging remain byte-identical at the function bodies. No generic platform
layer, dependency, authorization or calibration change is introduced.

The retained check compiles the actual module with warnings denied and runs seven
behavior checks through owned fake PATH executables. It asserts complete argv,
Unicode/spaces/shell characters as one literal operand, inherited versus null
stdio, reveal-parent behavior, and missing-executable logging without turning a
best-effort launch into a fatal result. Nothing opens a real app; the captured run
also denied network access. The same seven checks pass on the original functions
and the moved owner. A changed actual Quick Look flag is detected.

```sh
python3 -I -B .improve/structural-checks/check_external_open.py
```

`external-open.json` records the captured Darwin gates. The portable check selects
the real platform branch; its Linux xdg-open argv/parent/null-stdio checks execute
when run on Linux. The local std-only checks do not replace whole Mac compilation,
native UI tests or manual OS integration. Gate durations are not app-speed claims.
