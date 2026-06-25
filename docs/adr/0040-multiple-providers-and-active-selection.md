# ADR-0040 — Multiple configured providers with an active selection

- Status: Accepted
- Date: 2026-06-24

## Context

The host resolved exactly one LLM provider. `piwire.LoadConfig` produced a
single `Config` (one `ProviderName`, one `BaseURL`, one `KeyEnv`, one `Model`);
`settings.Settings` carried scalar `LLMBaseURL` / `DefaultModel`; the vault held
one key under the hardcoded name `OPENROUTER_API_KEY`; and the Settings tab's
provider panel (`gateway.LLMAdmin` + `ProviderSettings.tsx`) could edit only
that one endpoint and that one key. Choosing a non-default *adapter*
(`openai` / `anthropic` / `google`, ADR-0028) was env-only via `ARBOS_PROVIDER`
and unreachable from the UI.

Users want several providers configured at once (OpenRouter, a direct OpenAI
key, gm, a native Anthropic key) and to switch the active one from the provider
panel without re-typing credentials — and to see which provider is live, next
to the model selector in the composer.

## Decision

Introduce a **list of configured providers with one active selection**, kept in
the existing durable stores, switched through the existing graceful-restart
path. No new long-lived provider object, no engine change.

- **Data model.** `settings.Settings` gains `Providers []ProviderEntry` and
  `ActiveProviderID`. A `ProviderEntry` is non-secret: `{ID, Label,
  ProviderName, Endpoint, KeyVaultName, DefaultModel}`. Keys stay out of the
  file — each entry names its own vault entry (`KeyVaultName`, e.g.
  `LLM_KEY_<id>`), so multiple credentials coexist under distinct names and the
  secrets discipline (ADR-0016) is unchanged. Store CRUD (`AddProvider`,
  `UpdateProvider`, `RemoveProvider`, `SetActiveProvider`) is read-modify-write
  under the existing lock.

- **Resolution + lazy migration.** `LoadConfig` builds the effective provider
  set, then selects the active entry through the existing `providerSpecs` table.
  When `Providers` is empty (every pre-existing install, and a fresh one) it
  **synthesizes a single entry** from today's fields — `LLMBaseURL` +
  `OPENROUTER_API_KEY` + the resolved `ARBOS_PROVIDER` + `DefaultModel` —
  yielding a `Config` identical to the previous behavior, including the
  OpenRouter onboarding branch. An environment-configured provider
  (`ARBOS_PROVIDER` + `ARBOS_*_API_KEY`) surfaces as a read-only synthetic entry
  the UI cannot delete; env still wins over stored. There is **no eager rewrite
  on upgrade** — the file gains `providers[]` only when the user first acts in
  the new panel.

- **Downgrade safety.** Mutations keep mirroring the active entry into the
  legacy `LLMBaseURL` / `DefaultModel` fields. An older binary reading a
  new-world file ignores the unknown `providers` / `active_provider_id` keys
  (the settings package already drops unknown keys / zero-fills missing ones)
  and falls back to the mirror, running single-provider without error. Forward-
  and backward-compatible.

- **Switching = restart.** `SetActiveProvider` persists `ActiveProviderID` then
  reuses `host.Engine.RequestRestart()`; the provider is rebuilt whole through
  `LoadConfig` at the next idle moment — the same apply path the panel already
  uses for an endpoint/key save (ADR-0027 boot-id poll). No live multiplexing,
  no per-request routing, no second provider held in memory.

- **Seam + UI.** `gateway.LLMInfo` carries the provider list (with per-entry
  `keySet` and an env-sourced flag) and `ActiveProviderID`; `gateway.LLMAdmin`
  gains the CRUD + select operations behind new handlers. `ProviderSettings.tsx`
  becomes a list with an active selector, add/edit/remove, an adapter dropdown,
  and a per-entry endpoint + write-only key. The composer renders the active
  provider's label beside the model selector.

- **One panel for everyone (Option A).** A host that never adopted the new
  shape has an empty `settings.Providers`, so the UI would otherwise have no
  way to create the first entry — a dead end. Instead the web door's `Info`
  SYNTHESIZES a single row (id `"default"`) from the boot-resolved
  single-provider config whenever the stored list is empty, so the list is
  never empty for a configured-or-onboarding host and there is exactly one
  panel: a single-provider host is just "a list of length 1" with an Add
  button. The synthetic row is not persisted until the first mutation
  (add/edit/remove/activate), which seeds it as the first real `Providers[]`
  entry (keeping its legacy `OPENROUTER_API_KEY` vault binding) before applying
  the change — so adopting multi-provider mode never loses the existing config.
  The legacy single-provider form is removed.

## Consequences

- Several providers/adapters are configurable from the UI (not just env), each
  with its own coexisting key, switchable with one graceful restart.
- Migration is lazy and lossless: existing installs boot byte-identically and
  only adopt the new shape on an explicit save; downgrade keeps working via the
  legacy mirror.
- Switching costs a restart (one in-flight turn drains first), the deliberate
  trade for not touching the engine's provider handle, the per-provider model
  registry, or fallback (ADR-0023/0028). Live per-session routing is a possible
  later layer, not taken here.
- The composer's active-provider label is presentation derived from
  `LLMInfo`; it is not persisted on messages or events.
