---
cursor:
  subagentId: "bc-a4021d6f-dfd2-5778-aa40-def284137d1e"
---

# Secrets inventory — 1Password vault "Arbos"

**Default model backend:** OpenRouter — item ID `xbirrctuljw2m6aieoway2szom`, field label `credential`, env `OPENROUTER`. Read it with `op item get xbirrctuljw2m6aieoway2szom --vault Arbos --fields label=credential --reveal`.

**Infrastructure: machines and Cloudflare**

- SSH key for all agent logins: "Arbos Cloud Agents SSH Key" — `nlijfp36ed4aqbkp2svh2lefbi`, field `private key` (also `public key`, `fingerprint`, `key type`).
- ArbosLife (Affine SN120 validator box, `const@204.12.171.6`): `cxuhrsfxshwxfiuaidnysynbva`, fields `URL`, `username`, `port`, `aliases`, `ssh key ref`, `connect`, `notesPlain`. Production box.
- templar (CPU ops box, `const@204.12.168.71`): `odc5ecxekfvrv6hv4c2tmnznbe`, same fields minus `aliases`.
- chakanaone (local/VPN-only host): `6miilo2527ae5zfdnjk4awi75m`, same fields as templar. Not reachable from cloud VMs.
- Cloudflare account API token (full access, Pages + R2, env `CLOUDFLARE_API_TOKEN`): `pz4t7dalfdaldzi7el6rivobsi`, field `credential`.
- Cloudflare account ID (config, env `CLOUDFLARE_ACCOUNT_ID`): `mwjatqh662ozvrgaoocbnhxasa`, field `credential`.
- Cloudflare R2 S3 keys, account-wide, no IP lock: access key ID `5zos64ftkkhow3yd2ualigad44` + secret key `s6gnhchuq3xhzji34tn5aukhsm`, field `credential` on each; endpoint URL (config, env `R2_ENDPOINT`) `skikpa5qypq44rdm7tsm74dfea`, field `credential`.

Snapshot taken 2026-09-12 with the agent service account (`OP_SERVICE_ACCOUNT_TOKEN`). No secret values appear in this file. Only item IDs, titles, categories, tags, and field labels.

- Vaults visible to the service account: **1** — `Arbos` (vault ID `qqmqjwjr7kewyrlv4qphwmhlru`, 77 items).
- Field pattern for most `API_CREDENTIAL` items: `credential` (the secret), `notesPlain` (free text), `type`. Some also have `username`, `hostname`, `valid from`, `expires`. Rows below list only fields that differ from this default.
- "Env" is the environment variable name given in the item title. Use it when you export the value into a shell.

## Table, sorted by usefulness to an agent

Legend for "Use": GPU = rent GPUs, MACHINE = run code on a rented or owned box, MODEL = call an LLM API, STORE = object storage, DEV = code and deploy, COMMS = social or messaging, DATA = market or finance data, CONFIG = not a secret, DUP = same value as another item, DEAD = superseded or broken.

| # | Item ID | Title (short) | Category | Tags | Fields (labels only) | Service guess | Use |
|---|---|---|---|---|---|---|---|
| 1 | `eqrnnochebmpmeqgtnqnp3iogi` | Lium — GPU rental API key — env `LIUM_API_KEY` | API_CREDENTIAL | subnet120, validator-env | default | Lium (lium.io) GPU marketplace on Bittensor | GPU: rent eval/teacher/datagen pods |
| 2 | `uap3vvr7znka5r7do27wgbdhma` | Prime Intellect — API key — env `PRIME` | API_CREDENTIAL | subnet120, affine-env | default | Prime Intellect (hub with 1,646 envs + compute API) | GPU + evaluation environments |
| 3 | `xbirrctuljw2m6aieoway2szom` | OpenRouter — API key (no spend limit) — env `OPENROUTER` | API_CREDENTIAL | subnet120, affine-env, datagen-pod | default | OpenRouter (multi-model LLM gateway) | MODEL: any hosted LLM, incl. speech-capable models via OpenRouter |
| 4 | `qfc3qbigy37v3hevo73yahfkfu` | Chutes — API key — env `CHUTES` | API_CREDENTIAL | subnet120, affine-env, datagen-pod | default | Chutes (llm.chutes.ai, Bittensor SN64 inference) | MODEL: cheap open-source LLM inference |
| 5 | `rpjgmtwksitbflqn56sb4eluse` | Engy — API key #1, datagen teacher — env `ENGY` | API_CREDENTIAL | subnet120, affine-env, datagen-pod | default | Engy (api.engy.ai; GLM / Kimi models) | MODEL: teacher rollouts |
| 6 | `ydtoba3wd77dskzxu5uq5l3osq` | Engy — API key #3, eval-side teacher — env `ENGY_EVAL` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Engy | MODEL: evaluation |
| 7 | `hbmcxmszp7fthxom3kcsvd52la` | Engy — API key #2, research probes — env `ENGY_2` | API_CREDENTIAL | subnet120, affine-env | default | Engy | MODEL: research |
| 8 | `nlijfp36ed4aqbkp2svh2lefbi` | Arbos Cloud Agents SSH Key | SSH_KEY | cloud-agents | `public key`, `fingerprint`, `private key`, `key type`, `notesPlain` | SSH key pair for agents | MACHINE: log in to the boxes below |
| 9 | `cxuhrsfxshwxfiuaidnysynbva` | SSH — ArbosLife: Affine SN120 validator box — `const@204.12.171.6` | SERVER | cloud-agents | `URL`, `username`, `port`, `aliases`, `ssh key ref`, `agent key installed`, `connect`, `notesPlain` (+ empty console fields) | Owned Linux server (runs validator, dash, swarm, king-datagen) | MACHINE: long-running jobs; production box, be careful |
| 10 | `odc5ecxekfvrv6hv4c2tmnznbe` | SSH — templar: CPU ops box — `const@204.12.168.71` | SERVER | cloud-agents | same as row 9 minus `aliases` | Owned Linux server (Postgres, bots, monitoring) | MACHINE: CPU jobs, databases |
| 11 | `6miilo2527ae5zfdnjk4awi75m` | SSH — chakanaone: local/VPN-only host — `const@chakanaone` | SERVER | cloud-agents | same as row 10 | Jacob's local machine, not on the internet | MACHINE: only from VPN / self-hosted worker |
| 12 | `vvnyarkwampjl3diocn7n6vcqe` | GitHub — PAT (user unarbos; repo + write:packages; expires 2026-11-08) — env `ARBOS_GITHUB` | API_CREDENTIAL | subnet120, affine-env | default | GitHub | DEV: push code, open PRs, publish packages |
| 13 | `vxynhbmikcllahzj3vnswggzsq` | Hugging Face — token "Master2" (user unconst, RW) — env `HF_TOKEN` | API_CREDENTIAL | subnet120, validator-env, datagen-pod | default | Hugging Face Hub | MODEL/DATA: download or upload models and datasets (speech models included) |
| 14 | `222kjh7t34t5ysy5bm4o4pwupu` | Hugging Face — token "ARBOSNEWW" (user unconst, RW) — env `HF_NEW` | API_CREDENTIAL | subnet120, affine-env | default | Hugging Face Hub | MODEL/DATA: second token |
| 15 | `pz4t7dalfdaldzi7el6rivobsi` | Cloudflare — account API token, full access — env `CLOUDFLARE_API_TOKEN` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Cloudflare | DEV: deploy Pages sites, manage R2, DNS |
| 16 | `5zos64ftkkhow3yd2ualigad44` | Cloudflare R2 — S3 access key ID, account-wide, no IP lock — env `CLOUDFLARE_R2_ACCESS_KEY_ID` | API_CREDENTIAL | (none) | `credential`, `valid from`, `expires`, `notesPlain` | Cloudflare R2 (S3-compatible storage) | STORE: read/write all buckets from any IP (pair with row 17) |
| 17 | `s6gnhchuq3xhzji34tn5aukhsm` | Cloudflare R2 — S3 secret key, account-wide, no IP lock — env `CLOUDFLARE_R2_SECRET_ACCESS_KEY` | API_CREDENTIAL | (none) | same as row 16 | Cloudflare R2 | STORE: pair with row 16 |
| 18 | `skikpa5qypq44rdm7tsm74dfea` | Config — Cloudflare R2 S3 endpoint URL — env `R2_ENDPOINT` | API_CREDENTIAL | subnet120, affine-env, validator-env | default (`credential` is plain STRING) | Cloudflare R2 | CONFIG: endpoint for rows 16–17 |
| 19 | `mwjatqh662ozvrgaoocbnhxasa` | Config — Cloudflare account ID — env `CLOUDFLARE_ACCOUNT_ID` | API_CREDENTIAL | subnet120, affine-env, validator-env | default (plain STRING) | Cloudflare | CONFIG |
| 20 | `vsbu7y2srctn2x53vkrtfi32s4` | 1Password — service account token (Arbos vault, read-write, for agents) | API_CREDENTIAL | (none) | `credential`, `valid from`, `expires`, `notesPlain` | 1Password | DEV: same token agents already hold in `OP_SERVICE_ACCOUNT_TOKEN` |
| 21 | `5scwq5squerka4i2ftfrjun5mu` | X (Twitter) — ArbosApp bearer token (app-only) — env `X_BEARER_TOKEN` | API_CREDENTIAL | x-api, twitter, const_reborn, arbosapp | `username`, `credential`, `valid from`, `expires`, `hostname`, `notesPlain` | X API v2 | COMMS: read timelines, search |
| 22 | `rv35scdfxrwiqagrlg7o26edhe` | X (Twitter) — ArbosApp API key / consumer key — env `X_API_KEY` | API_CREDENTIAL | x-api, twitter, const_reborn, arbosapp | same as row 21 | X API (OAuth 1.0a) | COMMS: posting needs an access token not yet stored |
| 23 | `r3e42l27nyhhtc6eyeos4lp2eq` | X (Twitter) — ArbosApp API secret / consumer secret — env `X_API_SECRET` | API_CREDENTIAL | x-api, twitter, const_reborn, arbosapp | same as row 21 | X API (OAuth 1.0a) | COMMS: pair with row 22 |
| 24 | `kyx56yzyvhjeltydc25u4nsrvm` | X (Twitter) — HOW TO USE: @const_reborn API credentials index | SECURE_NOTE | x-api, twitter, const_reborn | `notesPlain` | Docs note | COMMS: read this first before using rows 21–23 |
| 25 | `hsxmduwdlkft3xoekt5um45mta` | Affine X | LOGIN | (none) | `username`, `password`, `email`, `notesPlain` (URL x.com) | X account login (Affine account) | COMMS: browser login, not API |
| 26 | `6uppsa55jnmrghyoijerikwdoy` | Discord — bot token "Arbos" — env `DISCORD_BOT_TOKEN_ARBOS_BITTENSOR` | API_CREDENTIAL | subnet120, affine-env | default | Discord bot | COMMS: post announcements in the Affine channel |
| 27 | `xbhuhj2wwwszhkmlyiqtqegciq` | Comms agent — cross-project inbox (how other agents use it) | SECURE_NOTE | comms, agents | `notesPlain` | Docs note for the Comms agent (Discord, WhatsApp, Telegram live; email next) | COMMS: interface docs, no secret |
| 28 | `mykpz6u3e32shqiylelqbfcf4q` | Telegram — api_id (Jacob's account) — env `TELEGRAM_API_ID` | API_CREDENTIAL | telegram, comms | `credential`, `hostname`, `notesPlain` | Telegram MTProto app | COMMS: used by Comms agent |
| 29 | `tcomvvbrlbts65hnjb46etb6n4` | Telegram — api_hash — env `TELEGRAM_API_HASH` | API_CREDENTIAL | telegram, comms | same as row 28 | Telegram | COMMS |
| 30 | `zgmd2dpkh4utwldxgodag54hri` | Telegram — phone number — env `TELEGRAM_PHONE` | API_CREDENTIAL | telegram, comms | same as row 28 | Telegram | COMMS: personal data, handle with care |
| 31 | `erfdxsnm2i46wlstbutksp6ufy` | Cloudflare R2 — access key ID, read-only on affine-private-models + affine-models — env `AFFINE_EVAL_R2_ACCESS_KEY_ID` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Cloudflare R2 | STORE: read model weights (pair with row 32) |
| 32 | `lnmncrndxc3u2zyl6bwj6d7qz4` | Cloudflare R2 — secret key, read-only, same buckets — env `AFFINE_EVAL_R2_SECRET_ACCESS_KEY` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Cloudflare R2 | STORE: pair with row 31 |
| 33 | `dhq5qdsxug3sfc5o7xsujo2hha` | Cloudflare R2 — access key ID, affine-data bucket only — env `DATA_R2_ACCESS_KEY_ID` | API_CREDENTIAL | subnet120, affine-env | default | Cloudflare R2 | STORE: corpus bucket (pair with row 34) |
| 34 | `53sjjyzmu7uphiyecsk72ip4ae` | Cloudflare R2 — secret key, affine-data bucket only — env `DATA_R2_SECRET_ACCESS_KEY` | API_CREDENTIAL | subnet120, affine-env | default | Cloudflare R2 | STORE: pair with row 33 |
| 35 | `xjmv54wwgd5crm4jmhupga5mku` | Cloudflare R2 — access key ID, affine-data (datagen pods publish traces) — env `ROLLOUTS_R2_ACCESS_KEY_ID` | API_CREDENTIAL | subnet120, rollouts-pod | default | Cloudflare R2 | STORE: pair with row 36 |
| 36 | `wnzx746e6casgnurtoyvdv4yaq` | Cloudflare R2 — secret key, affine-data (datagen pods) — env `ROLLOUTS_R2_SECRET_ACCESS_KEY` | API_CREDENTIAL | subnet120, rollouts-pod | default | Cloudflare R2 | STORE: pair with row 35 |
| 37 | `nmdt3ay4f4xb6lezwfy3sn3pum` | Cloudflare R2 — access key ID, account-wide, IP-locked to validator box — env `R2_ACCESS_KEY_ID` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Cloudflare R2 | STORE: works only from 204.12.171.6 |
| 38 | `ukommoiteam7nlrfyst35t3seq` | Cloudflare R2 — secret key, account-wide, IP-locked — env `R2_SECRET_ACCESS_KEY` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Cloudflare R2 | STORE: pair with row 37 |
| 39 | `b5fwtd42526nathn2eaafdj6ye` | Hippius S3 — access key — env `HIPPIUS_ACCESS_KEY` | API_CREDENTIAL | subnet120, validator-env | default | Hippius (s3.hippius.com, decentralized S3) | STORE: legacy corpus bucket |
| 40 | `nzbu6mub2x6urudvxy3amwx3ey` | Hippius S3 — secret key — env `HIPPIUS_SECRET_KEY` | API_CREDENTIAL | subnet120, validator-env | default | Hippius | STORE: pair with row 39 |
| 41 | `i3k43tr6252orhvfa2bpenpij4` | Affine — Ed25519 signing seed for dash.affine.io mailbox — env `AFFINE_MAILBOX_SIGNING_SEED` | API_CREDENTIAL | subnet120, affine-env, validator-env | default | Affine subnet (Bittensor SN120) | Validator identity; do not use outside Affine |
| 42 | `mw3a7rldsoad2dqe4za3cnyq4e` | Affine — X-Affine-Token shared secret, validator ↔ eval pods — env `AFFINE_EVAL_TOKEN` | API_CREDENTIAL | subnet120, validator-env | default | Affine internal auth | Affine eval pods only |
| 43 | `35vehktqwl7aftyyyagpjoqa6e` | TaoMarketCap — API key — env `TMC_API_KEY` | API_CREDENTIAL | subnet120, validator-env | default | TaoMarketCap (Bittensor market data) | DATA: subnet price data |
| 44 | `jodpc3xdz5nxfvaglishrqcp4a` | Kalshi — API key ID (crypto MM bot) — env `KALSHI_API_KEY_ID` | API_CREDENTIAL | (none) | `credential`, `valid from`, `expires`, `notesPlain` | Kalshi (prediction market) | DATA/trading: real money, pair with row 45 |
| 45 | `hbl66rdwtdgpha6bi6rneqg2vi` | Kalshi — RSA private key PEM — env `KALSHI_PRIVATE_KEY_PATH` | DOCUMENT | (none) | file `kalshi-crypto-private-key.pem`, `notesPlain` | Kalshi | DATA/trading: signing key for row 44 |
| 46 | `onbutkvub2pjxu4v4dwmo22jne` | Predexon — API key (Kalshi order-book snapshots) — env `PREDEXON_API_KEY` | API_CREDENTIAL | (none) | `credential`, `valid from`, `expires`, `notesPlain` | Predexon (predexon.com) | DATA: market snapshots |
| 47 | `dxqnow2slgv7rvipu67sm76a6m` | New New Openrouter | SECURE_NOTE | (none) | `notesPlain` (single token) | Probably an OpenRouter API key stored as a note (created 2026-08-06) | MODEL: unclear, see notes |
| 48 | `kjl52th626ruiez2y4gvp2smg4` | New New Cursor | SECURE_NOTE | (none) | `notesPlain` (single token) | Probably a Cursor API key stored as a note (created 2026-08-06) | DEV: unclear, see notes |
| 49 | `hzlj3vqtyybym6v3q3rfaawoty` | Rao Tsu header | SECURE_NOTE | (none) | `notesPlain` (single token) | Probably a Discord user authorization header for the "Rao Tsu" account (Comms agent) | COMMS: unclear, see notes |
| 50 | `nrzkhjsapwydc6npqgs4zixije` | Const discord header | SECURE_NOTE | (none) | `notesPlain` (single token) | Probably a Discord user authorization header for Jacob's "Const" account (Comms agent) | COMMS: unclear, see notes |
| 51 | `b2sp3v37xqwncru25pjcupnjoy` | Proton VPN | SECURE_NOTE | (none) | `notesPlain` (account email, OpenVPN username + password) | Proton VPN | MACHINE: VPN access, maybe to reach chakanaone (row 11) |
| 52 | `ci4fidwnpnioh5vjlghcattmla` | Targon — GPU rental API key; api.targon.com returns 410 Gone — env `TARGON_API_KEY` | API_CREDENTIAL | subnet120, validator-env | default | Targon (Bittensor SN4) | GPU: DEAD, endpoint gone |
| 53 | `xcelg3yh4evrx4kvk5kidrrw2a` | Config — datagen providers JSON — env `DATAGEN_PROVIDERS` | API_CREDENTIAL | subnet120, datagen-pod | default (plain STRING) | Affine datagen | CONFIG: which model each provider serves |
| 54 | `b63exh66helsd4mggfvfsgs3ae` | Config — bucket name "affine-sn120" — env `R2_BUCKET` | API_CREDENTIAL | subnet120, affine-env | default (plain STRING) | Cloudflare R2 | CONFIG (unused by code) |
| 55 | `celaexesbnzuiswymmgcjkdidu` | Config — HF dataset repo "unconst/affine-datagen-turns" — env `DATAGEN_HF_REPO` | API_CREDENTIAL | subnet120, datagen-pod | default (plain STRING) | Hugging Face | CONFIG (retired) |
| 56 | `hrtln6qfmqdwsnnjnk7g7y446y` | Config — HF dataset repo "unconst/affine-rollout-traces" — env `ROLLOUTS_TRACES_HF_REPO` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Hugging Face | CONFIG |
| 57 | `3fkaszh7ua3hhtrzawpxna5tgm` | Config — datagen agent timeout = 5400 s — env `DATAGEN_AGENT_TIMEOUT_S` | API_CREDENTIAL | subnet120, datagen-pod | default (plain STRING) | Affine datagen | CONFIG (legacy) |
| 58 | `hrnz7fletso4tsenzjwi2faszi` | Config — datagen batch size = 32 — env `DATAGEN_BATCH_SIZE` | API_CREDENTIAL | subnet120, datagen-pod | default (plain STRING) | Affine datagen | CONFIG (legacy) |
| 59 | `wlmmihtsl5gnclhamvgwvdxga4` | Config — rollouts shard "0/3" — env `ROLLOUTS_SHARD` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Affine rollouts pods | CONFIG |
| 60 | `n5xutpmujdhu3q2lo4kfoud7g4` | Config — rollouts language filter = "all" — env `ROLLOUTS_LANGS` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Affine rollouts pods | CONFIG |
| 61 | `qatip2oo2ec6up7t42kelk6xta` | Config — rollouts max containers = 24 — env `ROLLOUTS_MAX_CONTAINERS` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Affine rollouts pods | CONFIG |
| 62 | `nvr4252okvx57upqlgnzt577z4` | Config — rollouts batch size = 24 — env `ROLLOUTS_BATCH_SIZE` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Affine rollouts pods | CONFIG |
| 63 | `sjcqhq3pt73chkklvzoc4uq23i` | [DUP] OpenRouter — API key — env `OPENROUTER_API_KEY` | API_CREDENTIAL | subnet120, validator-env | default | OpenRouter | DUP of row 3 |
| 64 | `t7dt3mvv66lmefugiu67wdqw5q` | [DUP] Lium — GPU rental API key — env `LIUM` | API_CREDENTIAL | subnet120, affine-env | default | Lium | DUP of row 1 |
| 65 | `ogdx23nq2tm6amzqqsyxyx7usi` | [DUP] Hugging Face — token "Master2" — env `HUGGINGFACE` | API_CREDENTIAL | subnet120, affine-env | default | Hugging Face | DUP of row 13 |
| 66 | `n5a77zcnr6gjl6qztew4rhiz4y` | [DUP] TaoMarketCap — API key — env `TAOMARKETCAP` | API_CREDENTIAL | subnet120, affine-env | default | TaoMarketCap | DUP of row 43 |
| 67 | `scx72lijk5mh5zk3tcem2edf6e` | [DUP] Config — Cloudflare account ID — env `R2_ACCOUNT_ID` | API_CREDENTIAL | subnet120, affine-env | default (plain STRING) | Cloudflare | DUP of row 19 |
| 68 | `iwchcbqykxv34pjqemro3imhpq` | [DUP] Config — R2 S3 endpoint URL — env `DATA_R2_ENDPOINT` | API_CREDENTIAL | subnet120, affine-env | default (plain STRING) | Cloudflare R2 | DUP of row 18 |
| 69 | `pzqrzg3tddyhvt3pbyo5sbff2q` | [DUP] Config — R2 S3 endpoint URL — env `ROLLOUTS_R2_ENDPOINT` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Cloudflare R2 | DUP of row 18 |
| 70 | `ickqmtvql2z6qgjbdukrzoppyy` | [DUP] Config — HF dataset repo — env `ROLLOUTS_TURNS_HF_REPO` | API_CREDENTIAL | subnet120, rollouts-pod | default (plain STRING) | Hugging Face | DUP of row 55 |
| 71 | `ik7fqzjbl6wiqqwaexppmpsg2m` | [SUPERSEDED] X — bearer token (old standalone app) — env `X_BEARER_TOKEN` | API_CREDENTIAL | x-api, twitter, const_reborn, x-api-superseded | same as row 21 | X API | DEAD: v2 API rejects it |
| 72 | `lt46277rm4pggjbx4avrdeed5e` | [SUPERSEDED] X — API key (old app) — env `X_API_KEY` | API_CREDENTIAL | same as row 71 | same as row 21 | X API | DEAD |
| 73 | `a3kqe5aglydldokhzqfey2dqpe` | [SUPERSEDED] X — API secret (old app) — env `X_API_SECRET` | API_CREDENTIAL | same as row 71 | same as row 21 | X API | DEAD |
| 74 | `iichfenfv3o6oncwglb4nc2tmy` | [SUPERSEDED] X — access token (old app) — env `X_ACCESS_TOKEN` | API_CREDENTIAL | same as row 71 | same as row 21 | X API | DEAD |
| 75 | `xhlf76seugmwm3j6lggtikylnu` | [SUPERSEDED] X — access token secret (old app) — env `X_ACCESS_TOKEN_SECRET` | API_CREDENTIAL | same as row 71 | same as row 21 | X API | DEAD |
| 76 | `rlx4dlergpgljfs4texu56v7xu` | [SUPERSEDED] X — OAuth 2.0 client ID (old app) — env `X_CLIENT_ID` | API_CREDENTIAL | same as row 71 | same as row 21 | X API | DEAD |
| 77 | `7tymla6cnhenq6xskuhuaf5wai` | [SUPERSEDED] X — OAuth 2.0 client secret (old app) — env `X_CLIENT_SECRET` | API_CREDENTIAL | same as row 71 | same as row 21 | X API | DEAD |

### What is missing

No item exists for: OpenAI, Anthropic, ElevenLabs, Deepgram, or any other speech (STT/TTS) provider; RunPod, Lambda, Vast.ai, Hetzner, AWS, GCP, Azure. For speech work today the options are: open-source models from Hugging Face (rows 13–14) run on a Lium or Prime Intellect GPU (rows 1–2), or a speech-capable model reached through OpenRouter (row 3).

## How to read a secret

Titles contain em-dashes (`—`) that `op read` rejects. Always use the item ID.

```bash
# one field, printed to stdout (default field label is "credential")
op item get <ITEM_ID> --vault Arbos --fields label=credential --reveal

# into an env var without printing it
export LIUM_API_KEY="$(op item get eqrnnochebmpmeqgtnqnp3iogi --vault Arbos --fields label=credential --reveal)"

# secure notes and headers: the field label is notesPlain
op item get <ITEM_ID> --vault Arbos --fields label=notesPlain --reveal

# SSH private key (row 8)
op item get nlijfp36ed4aqbkp2svh2lefbi --vault Arbos --fields label="private key" --reveal

# document (row 45)
op document get hbl66rdwtdgpha6bi6rneqg2vi --vault Arbos --out-file /tmp/kalshi.pem

# list field labels of any item without values
op item get <ITEM_ID> --vault Arbos --format json | jq '.fields[] | {label, type}'
```

Rules: never `echo` a value, never paste one into a doc, log, PR, or chat. The `op` CLI is at `~/.local/bin/op` on this VM (v2.39.0); install with the zip from `cache.agilebits.com/dist/1P/op2/pkg/v<ver>/op_linux_amd64_v<ver>.zip` if missing.

## Items whose purpose is unclear

- `dxqnow2slgv7rvipu67sm76a6m` "New New Openrouter" and `kjl52th626ruiez2y4gvp2smg4` "New New Cursor": secure notes holding one bare token each, no tags, no description, created 2026-08-06. Guess: a fresh OpenRouter key and a Cursor API key. Unknown whether they are live or duplicates of row 3. Ask Jacob, or test the OpenRouter one against `/api/v1/auth/key`.
- `hzlj3vqtyybym6v3q3rfaawoty` "Rao Tsu header" and `nrzkhjsapwydc6npqgs4zixije` "Const discord header": one bare token each. Guess: Discord user-account `Authorization` headers used by the Comms agent (row 27 says it reads Discord as Jacob). These act as full account logins; treat as high-risk and do not use outside the Comms agent.
- `b2sp3v37xqwncru25pjcupnjoy` "Proton VPN": account email plus OpenVPN/IKEv2 username and password in the note body. Likely the way to reach `chakanaone` (row 11) from a cloud VM. Not confirmed.
- `hsxmduwdlkft3xoekt5um45mta` "Affine X": browser login for an X account, no notes. Unknown which handle; probably the Affine project account, not @const_reborn.
- `vsbu7y2srctn2x53vkrtfi32s4` 1Password service account token: stored inside the vault it unlocks. Useful only to bootstrap a new agent host.
- Rows 44–46 (Kalshi, Predexon): belong to a live trading bot on `mm-bot-prod-ash-01`, a host not in this vault. Real-money risk; not for general agent use.
- `ci4fidwnpnioh5vjlghcattmla` Targon: endpoint returns 410 Gone. Treat as dead until someone confirms a new API host.

## Addendum 2026-09-14 (layout worker, Cursor symmetry loop)

- The only Cursor item in the vault is `eagoyh7fpynfmzx4xm2agtqaim` "New New Cursor" (SECURE_NOTE, one 69-character token, created 2026-08-06). Row 48's id `kjl52th626ruiez2y4gvp2smg4` no longer resolves in the vault; use `eagoyh7fpynfmzx4xm2agtqaim`.
- Shape of the token (no value here): `crsr_` + 64 hex characters — a **Cursor API key** (the kind the Cursor Agent CLI and the Cloud Agents API take as `CURSOR_API_KEY`), not an account sign-in. It cannot sign the Cursor desktop app in: the desktop's login is a browser flow (cursor.com → email / Google / GitHub / login link → `cursor://` callback) and takes no API key. There is no email + password or login link for Cursor in the vault.
- Verified live 2026-09-14 17:28 UTC: with the token as `CURSOR_API_KEY`, `cursor-agent --trust -p "Reply with the single word pong."` answered `pong` from `/tmp/parity-proj` on the layout VM (Cursor Agent CLI `2026.09.10-fd3934a`, installed via `curl https://cursor.com/install | bash` to `~/.local/bin`). The Cursor desktop sign-in itself (authenticator.cursor.sh: Google / GitHub / Apple / passkey / email) still needs an account credential or login link that is not in the vault.
