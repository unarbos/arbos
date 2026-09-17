---
cursor:
  subagentId: "bc-26ac0e74-e578-597f-91c9-b8eee6e9218b"
---

# arbos.life website — build and deploy, 2026-09-13

**Live:** https://arbos.life (valid TLS, Let's Encrypt, expires 2026-12-12, auto-renews). Lighthouse 100/100/100/100 on `/` and `/install/`.
**PR:** https://github.com/unarbos/arbos/pull/125 (`cursor/arbos-life-website-218b` → `main`).
**Pages mirror:** https://arbos-life.pages.dev (project `arbos-life`; custom domains attached, pending DNS).
**Screenshots:** `media/website/` — `home-desktop-1440.png`, `home-mobile-390.png`, `install-desktop-1440.png`, `install-mobile-390.png`, `what-desktop-1440.png`, `docs-desktop-1440.png`.

## What was there before

- `arbos.life` A record → `204.12.171.6` (ArbosLife), DNS-only (grey cloud). `*.arbos.life` is a proxied wildcard (orange cloud) → ArbosLife port 80 (Cloudflare Flexible SSL → system Caddy).
- Port 443 was `forest-head-linux`, the Go-era "forest head" (device registry + lease server + the `curl https://arbos.life | bash` installer). Run by root systemd unit `/etc/systemd/system/forest-head.service` (User=const, Restart=always), not pm2. State in `/var/lib/arbos/arbos-forest` (symlinked from `~/.config/arbos-forest`): `devices.json` (last write Sep 4), `head.db`.
- Its wildcard cert (acme.sh, DNS-01 via a Cloudflare token saved in `~/.acme.sh/`) **expired 2026-09-13 17:27 UTC**, about 3.5 h before this work. `const` cannot use cron, so acme.sh never renewed. Plain `curl https://arbos.life` was already failing on TLS.
- There was no `www/` on any branch (the task mentioned one). `site/` on the Go `main` is the old Pages/wrangler installer site; left alone for the release worker.

## DNS and credentials — what I can and cannot change

| Thing | Finding |
|---|---|
| DNS host | Cloudflare zone `arbos.life` (id `9f03f556…`, nameservers matias/nancy, account "Jake@bittensor.com's Account"). Namecheap is only the registrar; DNS records are not there. |
| Vault Cloudflare token (`pz4t7dalfdaldzi7el6rivobsi`) | Account-owned token. Can list zones, create/deploy Pages projects, attach Pages custom domains. **Cannot read or write DNS records** (`10000 Authentication error` on `/zones/…/dns_records`). |
| Namecheap | Vault has `wqmriq3bhab45reardvvlip7ki` "New New Namecheap API Token" (secure note, one bare token, no username). Not used: Namecheap's API cannot edit records for a zone whose nameservers are Cloudflare, so it does not help here. It would only matter to change nameservers, which we should not do. |
| Cloudflare token on ArbosLife (`~/.acme.sh/…/arbos.life.conf`, `CF_Token`) | Has DNS edit on the zone (it issued the wildcard). I did **not** use it to repoint DNS; that is Jacob's call. It is the way to renew the wildcard if forest-head is ever brought back: `~/.acme.sh/acme.sh --renew -d arbos.life --ecc`. |

## Deploy path used

DNS could not be changed, so the site is served from ArbosLife, with Pages ready as the preferred path.

### ArbosLife (live now)

- `~/arbos-www/` — `site/` (copy of `www/`), `Caddyfile`, `bin/caddy` (copy of `/usr/bin/caddy` with `cap_net_bind_service`; the systemd unit also grants it), `update.sh` (pull `www/` from `main` and rsync into `site/`), `logs/`, `caddy-data/` (certs).
- `/etc/systemd/system/arbos-www.service` — enabled, User=const, `Conflicts=forest-head.service`. Caddy obtained the cert itself with TLS-ALPN-01 on :443 and renews it (port 80 belongs to the system Caddy and is firewalled to Cloudflare IPs, so HTTP-01 is disabled).
- `forest-head.service` — `systemctl disable --now`. Unit file kept. Backup at `~/archives/forest-head-backup-20260913T2125Z/` (binary, full config dir copy incl. `devices.json` and `head.db`, `cmdline.txt`, `README.txt` with restore note). Restore: `sudo systemctl disable --now arbos-www && sudo systemctl enable --now forest-head` (renew its cert first).
- `/etc/caddy/Caddyfile` (root) — appended one block: `http://www.arbos.life` → 301 `https://arbos.life{uri}`. Backup `/etc/caddy/Caddyfile.bak-20260913T2130Z`. Reloaded; `affine.io`, `app.arbos.life` unchanged.
- Nothing else touched. A stray pm2 entry `arbos-www` was created and deleted during the switch; `pm2 save` run.

### Cloudflare Pages (ready, waiting on DNS)

- Project `arbos-life`, production branch `main`, direct upload of `www/` at commit `1a33118`. `_headers` gives the same CSP and caching as the Caddyfile.
- Custom domains `arbos.life` and `www.arbos.life` added: status `pending` (validation method http; will flip when DNS points at Pages).
- Workflow `.github/workflows/pages.yml` in the PR deploys on push to `main` (paths `www/**`) and previews PRs. Needs repo secrets `CLOUDFLARE_API_TOKEN` (Pages: Edit) and `CLOUDFLARE_ACCOUNT_ID`. I did not set them (write to GitHub settings not authorised).

## What Jacob must do

1. **Choose the home for the site.**
   - Keep ArbosLife: nothing to do. Run `~/arbos-www/update.sh` after merging site changes (or wire it to a webhook).
   - Move to Pages (recommended; no server, CDN, auto TLS): in the Cloudflare dashboard, zone `arbos.life`, change the apex `A 204.12.171.6` to a **proxied CNAME `arbos.life → arbos-life.pages.dev`** and add **proxied CNAME `www → arbos-life.pages.dev`** (the explicit `www` record overrides the wildcard). Pages will then finish domain validation on its own. After that ArbosLife's `arbos-www.service` can be disabled.
2. **Repo secrets** for the workflow: `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (Settings › Secrets › Actions).
3. **Forest head**: if anything still relies on the Go-era `curl https://arbos.life | bash` or the device leases, say so; it can come back on another port, or the mesh hub replaces it. Otherwise it can stay off.
4. Optional: a Cloudflare token with `Zone:DNS:Edit` on `arbos.life` in the vault would let agents do 1 and the pending `hub-api`, `voice-api`, `kernel-api` CNAMEs themselves.

## Site content notes for the release worker

- Install text was written from code on `cursor/release-integration-52cd`: `~/.config/arbos/config.toml` keys (`provider`, `api_key`/`api_key_env`, `model`, `voice_url`, `voice_token`/`voice_token_env`), `arbos-kernel serve|run|worker`, `ARBOS_KERNEL_BIN`, `cargo install --path crates/arbos-kernel` then `desktop/`, iOS `scripts/gen-secrets.sh` + `xcodebuild`, `voice-server/deploy/run.sh`, `deploy/hub/worker.sh`. `LSMinimumSystemVersion` 11.0 → "macOS 11 or newer".
- `/docs/` links to `docs/design/{filesystem-state-design,cursor-vs-arbos-agent-model,desktop-call-mode-design,arbos-mesh-design,qa-loop-design}.md` on `main` — the store's document names. No branch has `docs/design/` yet; rename links if the release uses other paths.
- Download button: `href` is `releases/latest`; `assets/site.js` rewrites it to the `Arbos-*.dmg` asset URL once a release has one. Latest release today is still Go-era `v0.1.47`.
- Contact link points at new-issue on GitHub; swap for an email if Jacob wants one.

## 2026-09-17 — Jacob's review: download, favicon, minimal, image

**PR:** https://github.com/unarbos/arbos/pull/478 (`cursor/arbos-life-download-218b` → `main`). Live on https://arbos.life (ArbosLife) and mirrored to https://arbos-life.pages.dev.
**Stills:** `media/website/2026-09-17/` — `before-home-desktop-1440.png` (174 175 B), `before-home-mobile-390.png` (98 742 B), `after-home-desktop-1440.png` (215 316 B), `after-home-mobile-390.png` (103 951 B), `after-install-desktop-1440.png` (234 160 B).

### Download button

- Before: `href=releases/latest` → GitHub's "latest" is the Go-era `v0.1.47` (no Mac app). `v0.2.0` (with `Arbos-0.2.0-arm64.dmg`) is a **draft**: its assets 404 for anyone but the repo's writers. The JS asset lookup found no `.dmg` on `latest`, so the button opened a page.
- What is genuinely published for Mac: only the **dev channel** (tag `dev`, pre-release): `Arbos-<ver>-<build>-macos-arm64.zip`, Developer ID signed, notarized (run 35232241272: `status: Accepted`, "The staple and validate action worked!"), stapled; three builds kept, older ones pruned. **It is a zip of `Arbos.app`, not a DMG.** The dev workflow never runs `make dmg`.
- Now: button → `/download/mac` → 302 to the newest published build. Picker `deploy/www/mac-download.py`: stable release with a `.dmg` first (via API; drafts invisible), else newest dev build from `arbos-dev.json` (plain file, no rate limit; the anonymous API 403'd from this VM). Server: `~/arbos-www/refresh-download.sh` + `arbos-www-refresh.timer` (every 15 min, `User=const`, sudoers rule for `systemctl reload arbos-www.service` only) writes `download.caddy` and `site/download/mac.json`. Pages: the workflow runs the picker at deploy and appends to `_redirects`. JS reads `/download/mac.json`, points the button at the file, and prints "dev build 0.2.0+1509, 25 MB zip" beside it.
- Verified from this VM: `curl -JLO https://arbos.life/download/mac` → 302 → GitHub → `Content-Disposition: attachment; filename=Arbos-0.2.0-1509-macos-arm64.zip`, 26 495 826 B, `unzip -t` clean, `Arbos.app/Contents/_CodeSignature/CodeResources` and stapled `Contents/CodeResources` present, `CFBundleVersion 1509`, `LSMinimumSystemVersion 13.0`. Headless Chrome DOM after JS: `href="…/Arbos-0.2.0-1509-macos-arm64.zip"`. The timer already rolled the pick from 1478 to 1509 during the work.
- One file, not a choice: everything shipped is **arm64 only** (draft DMG `-arm64.dmg`, dev zip `-macos-arm64.zip`); the old caption "Apple silicon and Intel" was wrong and now says Apple silicon. A universal build needs `desktop/Makefile` to build `x86_64-apple-darwin` too and `lipo` the binaries before signing (macOS runner is arm64; `rustup target add x86_64-apple-darwin`), and the asset names to drop `-arm64`. Not done here; a release-worker change, unverifiable from this VM.
- Honest status: **the published artefact is the dev channel's zip.** A DMG on click needs either publishing `v0.2.0` or adding `make dmg` to `dev-channel.yml`.

### Favicon

Was: `<link rel="icon" href="/assets/favicon.svg">` only. Safari does not use SVG favicons and requests `/favicon.ico` → 404 (blank tab icon on Jacob's Mac); no `apple-touch-icon` (iOS home screen got a page thumbnail); the mark was my hand-drawn SVG, purple on a dark tile, not the app's icon (black trunk on a white squircle). Now rendered from `desktop/bundle/icon.png` following `artwork.swift` (tallest ink band = the mark, white→warm-grey tile, hairline edge): `favicon.ico` 16/32/48, `favicon-32.png`, `icon-192.png`, `icon-512.png`, `apple-touch-icon.png` 180 (opaque). All 200 on the live site.

### Minimal

One page: headline, one sentence, one button, screenshot, three one-line facts, footer GitHub · Docs · MIT. Cut: "How it works" cards, "Get started" section, `/what/` (301 → `/`), `/docs/` (302 → README), the TOC and callouts on `/install/`. Kept `/install/`: the zip path has steps a visitor needs (drag to Applications, microphone, model key). Nav: Install · GitHub.

### Image

Real and current: `arbos-desktop` build 1478 (`main` `f451cbe` at the time) from the dev channel's Linux tarball, run under Xvfb 1600×1000 with the OpenRouter key from the vault and a copy of this site as the project. Prompt: run a site-wide link check as a job and review the two pages. Frame: the agent's real answer, worker running, drawer showing Agents / Project (Link check: running, streaming) / Files. The agent's own review found a contradiction on the install page (Gatekeeper warning vs notarized builds); fixed.

### Checks

- Benchmark claims: none on the site (grep of `www/` for bench/SWE/Codex/%/22/24: only CSS percentages). README on `main` has none either.
- Lighthouse live: 98 / 100 / 100 / 100 (CLS 0.084 from the caption text changing when JS fills it).
- Server: `arbos-www.service` active, `arbos-www-refresh.timer` next run 14:56; `/etc/sudoers.d/arbos-www` (one command). `forest-head.service` still disabled.
