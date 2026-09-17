---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Settings › Model fields — PR #115, branch `cursor/settings-model-fields-b027`

- `key-field` (masked) + `key-save`: typed key → provider check → `config.toml`; field cleared. `key-paste` unchanged.
- Custom provider → `base-field` + `base-save` write `api_base`.
- `model-search` filters the catalog live; `model-pick-N` chips; `model-use-typed` when nothing matches.

## Try

- Type a key, then switch provider before saving: the field keeps the text (it is only cleared on Save). Say if it should clear.
- Paste a key with a trailing newline into the field → Save trims it.
- A key with a space → "That is 1 lines with spaces; a key is one word." (wording could be better; tell me).
- Search "gpt", pick a chip → `config.toml` `model =` updates; header line under Default model shows it.
- Base URL without scheme (`example.test/v1`) → "must start with http:// or https://".
- Switching Custom → OpenRouter resets `api_base` to OpenRouter's (pre-existing behaviour of `set_provider`); the typed base is lost. Expected, but note.
- Screen reader / driver: the masked field's stars are not in the accessibility text; the driver cannot read the typed key (good).
