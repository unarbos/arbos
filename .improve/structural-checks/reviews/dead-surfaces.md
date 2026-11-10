# Client G: private Mac surplus surfaces

The reference scan covers 1,275 tracked source/test/rig/config/documentation text
files plus current untracked Mac source. Private evidence copies, binaries and
.env files are excluded. `reference-audit.json` records locations without source
payloads. Mac is a binary target, not a published library exposing these methods.
There are no relevant feature/cfg/rig callers, module aliases or wildcard imports.

`candidate.patch` changes only private copies of app.rs, profiles/pane.rs and the
outdated design paragraph. The patch applies to the live tree, but is not applied
by this task. No tests, authorization code or transport implementation was removed.

| Original G finding | Disposition and evidence |
| --- | --- |
| CallTarget / AppState::start_call | Removed enum and obsolete pane entry method. Four CallTarget locations are its declaration, signature and two internal match arms. There is no caller. Current composer/chat flows use start_call_in_chat; that function and everything following it, including its tests, remain byte-identical. The voice panel's own start_call method is a different receiver and stays. |
| notify_approval_requested | Retained with its NotifyKind variant. The two references are the method and a notification-kind comment; no actual gate caller or equivalent background alert is established. The concrete missing contract is detailed below. This is a disposition, not an unexplored scan blocker. |
| Message-grant picker / future UI wrappers | Removed unused open/set/remove UI methods, target field/initialization/resets and unreachable pick-person branch. The target was assigned Some only by the uncalled opener. Set/remove wrappers each appear only at their definitions. Project grant data/DTOs, ClientCommand variants, relay request handlers and backend consent enforcement stay. Updated the historical design paragraph accordingly; a future UI should use that preserved API. |
| Profiles pane DSL / name renderer | Removed only Section::Picker (declaration and renderer arm; no constructor), unused pane::name, its unused TEXT_TINY import and broad dead-code allowance. All nine pane imports lack a name alias/wildcard; no Section aliases/variant imports occur. Kept Title/Card/Reveal/Empty/Footnote/Custom, all constructed by current pages, and the live Picture/Block structure. |

## Approval-notification contract retained for review

The generic notifier applies the current project lookup, chat visibility, mute,
streaming and coalescing rules. Its approval hook has no producer. Existing
needs_input handles open questions / legacy AskUser status, not pending approval
parts; the Mac push dispatch does not turn SecretRequest into this notification.
Neither path establishes an equivalent approval-request alert.

Before wiring or replacing this hook, pin the actual event/lifecycle contract:

1. Define which newly pending approval transition emits an alert, using stable
   request/project/chat identity. Full snapshots, backfill, reconnect/replay and
   repeated pending updates must not invent duplicate new requests.
2. Resolve decided/revoked-before-delivery races and account/project lifetime;
   preserve the existing visible/muted/streaming/coalescing behavior. A notification
   must open the owning chat and must never itself authorize tool execution.
3. Define a safe notification summary; do not expose raw command/secret payloads.
   Keep the actual approval decision and authorization on the existing backend.

Recommended default is to keep the hook until a tested producer/replacement is
established, rather than delete the last declared approval-alert surface as a
lint cleanup. The missing producer is an integration/policy gap requiring its
own behavior pin; absence of tests alone is not presented as an external blocker.
The entire existing notification gate/pipeline/hook is byte-identical here.

## Verification and limits

Rustfmt syntax/check passes for both changed Rust files. Exact reference/alias
inspection and preserved-source checks pass. All existing app/pane tests are
unchanged; no runtime test or speedup is claimed for deleting unreachable code.
Full Mac compiler/GUI integration still requires the native gate. This is a
complete disposition of all four old G items, with one deliberate security-
adjacent retention and no deletion of consent controls or live call behavior.
