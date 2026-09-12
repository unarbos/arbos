---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: OpenRouter-first backend (PR #6)

From the features agent. PR: https://github.com/unarbos/arbos/pull/6 (branch `cursor/openrouter-default-provider-b027`, base `rust`).

## What I built

- `~/.config/arbos/config.toml` gains `provider = "openrouter" | "openai" | "custom"`. Default is OpenRouter. Empty `api_base` / `api_key_env` / `model` fall back to the provider's defaults. A file with only `api_base` is read by that URL.
- OpenRouter requests carry `HTTP-Referer` and `X-Title`.
- A turn with no key now writes a failed notice to the transcript and closes the turn (before: silent stderr).
- `arbos-kernel setup`: interactive (tty) or flags (`--provider --base --model --key-env --key-stdin --yes`). Checks the key, lists models, makes one real call, saves the file at mode 0600.
- Desktop Settings › Model: provider control, key source row with **Paste key** (reads clipboard, validates, saves) and **Forget**, default-model pills, Reveal config file.

## How to exercise it

```bash
cargo build -p arbos-kernel
export XDG_CONFIG_HOME=$(mktemp -d)                 # isolates the config
./target/debug/arbos-kernel setup --yes             # uses $OPENROUTER_API_KEY
./target/debug/arbos-kernel setup                   # interactive; drive it through a pty
./target/debug/arbos-kernel serve /tmp/place        # then send a user frame to the port in .arbos/kernel.json
```

Desktop: `cd desktop && cargo build`, launch with the Python driver (`desktop/driver/arbosdriver.py`, `xdg=` for a private config), `app.action("cydonia::OpenSettings")`, `use_window` the settings window, `click("section-1")`. Element ids: `provider-0..2`, `key-paste`, `key-forget`, `model-pick-N`, `config-reveal`. Clipboard under Xvfb: `xclip -selection clipboard -i` (redirect its stdout/stderr to /dev/null or the caller hangs).

The OpenRouter key is in the vault: item `sjcqhq3pt73chkklvzoc4uq23i` (env `OPENROUTER_API_KEY`). Never print it. Do not `cat` config.toml after a paste; grep for `^model|^provider` only.

## What could break — attack here

1. **Config compatibility**: no `provider`; `provider` that disagrees with `api_base`; `custom` with no `api_base`; `api_key = ""`; unknown keys (must error, not ignore); a file with only a comment; a non-UTF8 file.
2. **Key leaks**: search `kernel.log`, `transcript.jsonl`, `trace/*.json`, setup stdout, and the settings window for the key text. Server 401 bodies are echoed to the user; OpenRouter's does not contain the key, OpenAI masks it; a custom endpoint might not.
3. **Race**: paste a new key while a turn runs; the running turn keeps the old `Provider`, the next turn reads the file. Two writers (setup + desktop) at once: tmp+rename, last wins; check no torn file.
4. **Clipboard**: trailing newline, quotes around the key, multi-line, 10 MB text, binary.
5. **Setup in odd places**: non-tty without `--yes` (behaves as `--yes`); provider down (20 s / 60 s timeouts); `--key-stdin` with empty stdin; `--model` not in the list under `--yes` (must refuse).
6. **The notice path**: remove the key, send a prompt, confirm the chat shows the notice and the kernel does not replay the wake on restart (`needs_serve`).
7. **Desktop model catalog**: with no key, the composer picker must fall back to the gateway list or show an error, not hang.

## Known limits (not bugs)

- No masked text field in the desktop; key entry is clipboard-only there (backlog U-10).
- `custom` provider with an OpenRouter URL still gets OpenRouter headers (inferred from the URL); harmless.
