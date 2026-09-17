---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Data policy + free tag — PR #171, branch `cursor/training-tier-tag-b027` (on `main`)

T3-12, scoped to what OpenRouter exposes: `data_policy = "deny"` (or `"zdr"`) in config.toml puts `provider.data_collection = "deny"` (+ `zdr: true`) on every OpenRouter request; the composer's model picker tags free endpoints "free · may train" (`model-free-tag-<ix>`).

Scenarios (need the OpenRouter key):
- `data_policy = "deny"` and a `:free` model → OpenRouter should refuse or route to a compliant provider; the turn's failed notice (if any) names the policy. Trace (`trace = true`) shows `provider.data_collection` in the request body.
- `data_policy = "zdr"` with Claude → request carries `zdr: true`; works with Anthropic's ZDR endpoint.
- No `data_policy` → request body has no `provider` key (unchanged).
- Picker: rows ending `:free` show the tag; paid rows do not; tooltip names `data_policy`.

Unit: `provider::cache_tests::the_data_policy_becomes_openrouters_provider_routing`.
