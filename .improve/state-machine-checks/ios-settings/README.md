# Settings and confirmation ownership checks

Run on macOS (Foundation/Swift only):

```sh
python3 .improve/state-machine-checks/ios-settings/run_all.py --source-root "$PWD"
```

Use `--output /tmp/settings-check-new` to retain the generated fixtures and logs in a fresh directory. The runner extracts the actual production methods/closures and includes real WorkLifetime and account acceptance. It uses local profile/connector collaborators and a recording View/Binding boundary for the generic confirmation helper. Every executable runs with a clean environment and network denied. No app, SwiftUI renderer, device, account, provider or service is launched.

The final repeat passes58 checks in six groups:

- Profile save7: old queued action, retired/cancelled completion, owned busy cleanup, normal success/error and invalid input.
- Delayed agent navigation6: queued/stale/cancelled/nil origin and current/fixture controls.
- Token form7: rendered origin and stored form origin, replacement form, provider binding, trimming/empty/unpaired controls.
- Origin acceptance14: actual account lifetime, same-owner refresh, retirement/re-adoption, fixture and explicit local unpaired policy, relay/ABA/cancellation refusal.
- Anchored connector dialog7: captured selection, stale actions/cancel/binding, reentrant replacement and normal controls.
- Shared item confirmation17: observed payload/entity ID, account admission before clearing, nil→late item, same-ID payload, duplicate action, stale cancel/binding, cancellation, current labels/style and reentrant selections.

These are selected-source controller tests. They do not establish SwiftUI rendering/lifecycle behavior or full SDK compatibility; the project SDK build is a separate gate. The harness replaces UI state storage and effect collaborators while retaining actual method bodies and helper logic.

## Ownership policy

Authenticated callbacks retain AccountWork from their rendered/form context, guard entry and publication, and supply its owner to async model helpers. Profile forms retain their initial work; their owned busy flag can clear on same-account cancellation while stale owners cannot clear replacement state. Token drafts and confirmation payloads retain their origin; pending Settings confirmations have per-presentation UUIDs.

SettingsOrigin distinguishes authenticated work, fixture, unpaired relay/lifetime and unavailable capture. Nil work is never account permission. Existing global/local actions explicitly allow an unchanged unpaired origin; privacy's fixture/unpaired preference stays local. Async reads carry a nonnil existing lifetime receipt, including fixture seeding. Device/API permissions remain enforced by the model and relay.

The item-confirm helper's ID is the identified-entity contract. It always performs the observed payload, never a later binding value. Account-scoped callers must provide `isCurrent` so a stale account cannot clear a same-ID replacement before the perform guard. Callers needing distinct operations for repeated entity IDs use a presentation UUID, as Settings does. Clearing happens before the effect, so an effect's new pending selection survives.

All six item-confirm call sites are inventoried in CALLERS.json. System notification permission, app updates, appearance and text-size preferences keep their existing global policy. UI geometry, physical calibration, strings and shared ConfirmSpec button/style policy are unchanged.

Canonical unchanged-source replays fail29 of44 controller/helper cases and retain15 positive controls (BASELINE.json). The final58 include14 new explicit-origin policy checks. Initial harness compile mistakes are not semantic failures and are excluded from these counts. Same-source SDK compilation and the caller-source inventory remain separately recorded by the encompassing phase report.
