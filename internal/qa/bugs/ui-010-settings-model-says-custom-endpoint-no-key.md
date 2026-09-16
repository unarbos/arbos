# ui-010: Settings › Model shows "Custom endpoint" and "API key: None yet" while the kernel is answering through OpenRouter

status: new
severity: low (misleading, no functional loss)
scenario: internal/parity/ui_pass.py phase W (`section-1`, `provider-*`, `key-*`)
found: UI QA pass 2026-09-13, both branches
feature: Settings › Model (`desktop/src/view/settings/model.rs`)
fingerprints: none

## Repro

`~/.config/arbos/config.toml`: `model = "openai/gpt-5.4-mini"`, `api_base = "https://openrouter.ai/api/v1"`, `api_key_env = "OPENROUTER_API_KEY"`, key in the environment. Chats answer. Open Settings › Model.

## Expected

Provider OpenRouter (or "custom endpoint at openrouter.ai"), API key: present (from `OPENROUTER_API_KEY`), the model list for that provider.

## Actual

Provider pill **Custom endpoint** ("provider = custom needs api_base in config.toml"), **API key: None yet. Copy it, then paste it here. Or set ARBOS_API_KEY.**, Default model list empty (the `model-pick-*` rows vanish once the page settles). The page reads the config file, not the kernel's `provider` frame (PR #85 added `provider {provider, model, key, source}` which has exactly this information).

## Suspected location

`settings/model.rs`: provider detection from `api_base` presence; key presence from the desktop's own store only.

## Evidence

- `media/qa-ui/integration-67dcb85/054-section-1.png`, `media/qa-ui/integration-67dcb85/055-model-pick-0.png`
- `media/qa-ui/pr71-afa582a/072-section-1.png`, `media/qa-ui/pr71-afa582a/073-model-pick-0.png`
