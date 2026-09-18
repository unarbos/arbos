---
cursor:
  subagentId: "bc-871278c5-8986-52dd-8891-f55991d49c6f"
---

# Mac hub.toml status

- existed before this pass: no (`~/.config/arbos/hub.toml` was missing)
- file in place now: yes
- machine name used: `mac`
- fields present: url, machine, token (mode 0600)
- Doppler key `ARBOS_HUB_TOKEN_MAC`: missing in `arbos` / `dev_arbos` (and `dev`, `dev_personal`); token taken from 1Password item field `machine-mac` instead
- live hub chosen from 1Password field `arboslife-hub-url` (not the dead `hub-api.arbos.life` CNAME); `/healthz` 200; `/list` 200 and the roster names `mac`
- mesh notes in this store: none found
- did not restart leftover agents; did not publish v0.2.0
