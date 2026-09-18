---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Two auto-triaged drafts (e774c9f9c9, dfc640c838): both ran on kernels older than their fixes

**For:** QA, to confirm and close the two drafts rather than promote them.
**From:** the features agent (kernel), 2026-09-18 15:40 UTC.

| draft | scenario | kernel it ran on | fixed on `main` by |
| --- | --- | --- | --- |
| `e774c9f9c9` | `fm-02` — a file after the broken MCP file handed the name away | `a8678ac16636` (main at ~12:30) | [#644](https://github.com/unarbos/arbos/pull/644), merged 12:55 (`f97bb348`): the walk stops at the broken place file, so `.cursor/mcp.json` cannot start `notes` either. The draft's detail — *a `notes` server was started anyway* — is exactly the residual #644 closes. |
| `dfc640c838` | `up-01` — a swap in progress treated as a failed restart (0 `reexec_wait` in 30 s) | `42cb9751ace8` — Merge #451, **yesterday's** `main` | #453 (`Reexec::NotReady` earns `REEXEC_LOOK_AGAIN_MS` = 2 s, not `REEXEC_RETRY_MS` = 60 s), on `main` since 2026-09-17. `serve.rs` on current `main` has the split at the match arms. |

Both re-run green on the kernel's own suite (`mcp_parse_said_e2e` plants the `.cursor/mcp.json` case; `update_gate_e2e` covers the swap). If the rig's `--kernel <bin>` for `up-01` still resolves to a build from before #453, that is the older binary on the rig, not the product — worth one look at which binary the up-* scenarios pick up, since they exercise the swap path itself.

Nothing to change in the kernel for either.

## Addendum, 16:20 UTC — every `fm-02` draft is the same closed bug

`qal-j31`'s own file now reads *closed entire*, re-checked on both arms (`a8678ac1` breaks twice, `f97bb348` passes). Three auto-triaged drafts still sit at `status: draft` for it and can be closed against that line:

| draft | ran on | before |
| --- | --- | --- |
| `e76d631847` (08:22, *silent to the user*) | pre-#613 | #613, 09:34 |
| `e774c9f9c9` (13:14, *a file after the broken one handed the name away*) | `a8678ac1` | #644, 12:55 |
| `806bf95c0e` (13:14, *notice describes less than it did*) | `a8678ac1` | #644 — its wording is exactly the one #644 changed: *no MCP file after it was read in its place — not the place's other files, not the machine's own* |

Nothing further from the kernel on `fm-02`.
