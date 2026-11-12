# Spec: Inference Payments — Gamma Credits In, One Ledger

Status: **Proposal, revision 2** (2026-11-11). Nothing here is implemented. There is **no owner spec** behind it: "proper inference payments" was asked for, this is our reading of it, and the decisions in §10 are unanswered.

**What changed in this revision.** Revision 1 (`fc6d71977`) centred on card top-up through Stripe, with TAO as a late optional rail. This one recommends a single design: users fund inference with Bittensor **gamma** credits and the relay is the service's **spender**. Stripe, gifting, sponsored rooms and the Bittensor upstream provider are out of this revision (§7); TAO cheques are sidelined (§7).

Sources read for this revision: the Bittensor docs pages `guides/gamma`, `guides/services`, `guides/signed-requests`, `tx/spend-gamma`, `query/gamma-deposit` (www.bittensor.com/docs, 2026-11-11), and the code named in §1. What those pages leave open is listed in §8.

---

## 0. Goal & Core Decisions

1. **Keep the one ledger and the one cap.** `credit_ledger` and the gate in `crates/arbos-relay/src/credits.rs` stay the only meter, for every user. Gamma is a way for value to come *in*, not a second accounting system.
2. **Gamma is the funding rail.** A user holds a gamma deposit on one subnet; the relay, as that deposit's spender, calls `spend-gamma` to repay what the ledger has metered. 1 gamma repays $1.00 of metered cost.
3. **Settle in arrears, with a small fixed exposure.** The relay never spends gamma it has not metered. A linked user may run `E` (default 1 gamma) below zero, plus the call in flight, before the gate refuses.
4. **The relay holds one narrow key.** A `GammaSpend` proxy key, which per the docs can make no other call. No user key, no coldkey, no price feed.
5. **Users without a deposit are unchanged**: welcome grant and admin top-up, as today.

---

## 1. Starting Point: What Exists

| Piece | Where | State |
| --- | --- | --- |
| Metering proxy; boxes hold no provider key | `crates/arbos-relay/src/proxy.rs` | On `main`. Every call gets a `req_id`; `credits::check_call` before, `credits::charge_call` after |
| Per-user ledger | `credit_ledger(user_id, at, kind, usd REAL, ref, note, actor)` in `db_accounts.rs` | On `main`. Kinds `grant`, `inference`, `voice`, `machine` |
| Spend gate | `credits::check`: kill switch, balance, one daily limit | On `main`. Returns `Ok` on a DB error |
| Payer | `credits::payer()`: the principal's user, else the project owner | On `main` |
| Money in | `ARBOS_DEFAULT_GRANT_USD` (default 100) and `PUT /admin/users/{id}/credits` | Admin-only. No purchase path |
| Client surface | `GET /me` → `credits` block; iPhone Settings › Credits; Mac the same page on `cursor/mac-credit-balance-53ee` (`84f458bd9`) | Both clients render what the relay reports. Gamma follows that: the relay reports it, the clients show a line |
| Wallet | `crates/arbos-wallet` (#2932), `PUT /me/wallet` link proof (#2933), iPhone wallet (#2935) | **Open PRs, not on `main`.** One 12-word phrase → sr25519 account (`Seed::address`), JSON-RPC over HTTPS, `Net::{Main, Test}`. Device-only by design |
| TAO cheques | `arbos_wallet::chain`, iPhone `Wallet.chequesEnabled = false` (`fab447c65`) | Off in the app |

Nothing in the repository mentions gamma, a netuid, or a subnet of our own.

---

## 2. Ledger Hardening (Prerequisite)

| Today | Problem once value moves | Change |
| --- | --- | --- |
| `usd REAL`, balance = `SUM(usd)` | Float drift | Add `micros INTEGER NOT NULL` (1 USD = 10^6), backfill `round(usd * 1e6)`, read `SUM(micros)`. 1 micro = 1000 gamma units exactly |
| No idempotency key | A re-read chain event double-credits | Add `idem TEXT` with a unique partial index. Gamma credits use the settlement `reference` |
| `credits::check` returns `Ok` on a DB error | A locked DB is free inference | Fail **closed** for metered accounts (503). `paircode` stays ungated |
| `charge` clamps an overshoot at the balance | The clamp hides what is owed | Debit the true cost. The balance may go below zero; the gate (§3.3) bounds how far |

One new kind: `gamma` (a positive row). The ledger stays append-only.

**Sequencing:** #2951 rewrites settlement in `credits.rs` and `db_accounts.rs`. Land or decline it first.

---

## 3. Money In: Gamma

What the docs define: gamma is a USD-valued credit of one subnet (10^9 units per gamma), minted by burning that subnet's alpha, held in a deposit keyed `(netuid, hotkey)` and controlled by a coldkey. The deposit names one **spender**, who can take from it up to a daily cap. Gamma cannot move between coldkeys, cannot be redeemed, does not expire, and is destroyed if the subnet is dissolved.

```
user coldkey ── mint-gamma (--service-hotkey <ours>, daily cap) ──▶ deposit (netuid, hotkey)
device ── PUT /me/gamma, signed by the deposit hotkey ──▶ relay ── reads the deposit, stores the link
box ── /proxy/v1/* ──▶ relay ── check → upstream → debit ledger (as today)
relay ── spend-gamma [(hotkey, owed, reference)…] ──▶ chain ── GammaSpent ──▶ relay ── ledger +gamma row
```

### 3.1 Keys

- **Coldkey** = the wallet account that exists today (`Seed::address`). It mints, sets the cap and can pause or re-point the deposit at any time. It never signs anything for the relay.
- **Deposit hotkey** = a new sr25519 key from the same phrase by Substrate hard derivation, `<phrase>//arbos//gamma`. The docs ask for a fresh hotkey per deposit and never a coldkey; deriving it means the one phrase still recovers everything. It stays on the device and signs exactly one kind of request (§3.2). It never signs a chain transaction.
- **Spender** = the account our published service names (§3.5). The relay holds only its `GammaSpend` proxy key.

Inference calls come from boxes, not devices, so the hotkey cannot sign each call and is not given to a box. It signs the link once; after that the existing bearer tokens and `credits::payer()` decide who pays, as today.

### 3.2 Linking a deposit

`PUT /me/gamma`, device principal, plus the Bittensor signed-request headers (`btauth/1`) made with the deposit hotkey, receiver = our service hotkey. Body, exactly: `{ "v": 1, "action": "gamma.link", "user_id", "netuid", "hotkey", "genesis", "expires_at" }`. Verification is §6. On success the relay reads the deposit at a finalized block and requires that it exists, that `spender` is ours and that its cap is absent or at least `E`. It then stores `gamma_links(user_id PRIMARY KEY, hotkey UNIQUE)`.

`DELETE /me/gamma` needs only the device principal: unlinking can stop spending, never start it, and must work for someone who lost the phrase.

`GET /me` → `credits` gains `gamma: { spendable, daily_cap, usable }`; both clients add one line to Settings › Credits.

### 3.3 The gate

`owed = max(0, −balance)`. A call is allowed when the balance is above zero, or when all of these hold: the user has a link; `owed < E`; the last read of the deposit is at most 10 minutes old; and it showed our spender and `spendable_gamma ≥ E`. Everything else is today's 402. `E` is what our service document publishes as `max_spend`, which the docs define as the amount a customer must keep spendable.

A welcome grant is therefore used before any gamma (decision 6).

### 3.4 Settling

One task, one signer, once a minute:

1. For each linked user with `owed − pending > 0`, in one SQLite transaction insert `gamma_settlements(id, user_id, hotkey, units, reference, state='pending')`, with `units = (owed − pending) × 1000` and `reference = SHA-256("arbos/gamma-settle/v1\n" ‖ id)`. Owed derives only from metered debits, so **the relay cannot ask for more than it metered.**
2. Put up to 512 entries in one `spend_gamma(netuid, spends)` call, sign it as a **mortal** extrinsic with the proxy key, and store the signed bytes and the era's last block **before** submitting.
3. Submit. A retry only ever resubmits the stored bytes; the account nonce makes those land at most once. The docs are explicit that the chain does not deduplicate by reference ("an entry sent twice spends twice"), so a new extrinsic for the same debt is built only after step 5.
4. Follow finalized blocks. A `GammaSpent` whose netuid, hotkey, amount and reference all equal a pending row writes one ledger row (`kind='gamma'`, `+micros`, `idem=reference`) and marks it settled. An event carrying our reference with any other field different stops the settler and alerts.
5. A row whose era has passed in the finalized chain with no event is marked expired; the next tick starts again from step 1 with a new id.

One entry per user per tick, not per request: an agent makes many small calls and each entry is a fee. The per-request trail stays in the ledger (`ref` = `req_id`); the settlement row lists the ledger ids it covers.

### 3.5 What the relay needs on chain (owner actions, not code)

A service published on the subnet by its owner hotkey or a validator with a permit, naming the spender; a JSON document at its URL with `payments: gamma`, `max_spend = E`, `signature: "btauth/1"`; a `GammaSpend` proxy from the spender to the relay's key; TAO on that key for fees. The docs recommend a multisig spender so a coldkey swap never pauses spending.

---

## 4. Pricing & Who Is Paid

- 1 gamma repays $1.00 of ledger debit. `ARBOS_INFERENCE_MARKUP` (default `0.0`) applies to the debit, in `Meter::usd`, as proposed before.
- **Spent gamma pays the spender nothing.** The docs: the burned alpha's value goes to the subnet's alpha holders. The relay's upstream bills (OpenRouter, OpenAI, Anthropic) are still paid in dollars by the operator. Gamma only makes sense if whoever pays those bills is content to be repaid through the subnet's alpha. This is decision 2.
- A model with no known price and no reported cost is refused, not guessed (unchanged from revision 1).

---

## 5. Failure Handling

| Event | What the relay does |
| --- | --- |
| `GammaSpendSkipped` (no deposit, other spender, over balance, over cap) | Mark the row skipped, re-read the deposit. Nothing else: the gate already refuses a deposit that no longer shows our spender and `spendable ≥ E`. The debt stays on the ledger and is retried when a read shows it fits. Worst case loss is `E` plus the one call in flight, per account |
| Daily cap exhausted (`spendable < E`, balance above it) | 402 `daily_cap`, the existing shape. The cap refills linearly over 7200 blocks; the next read lifts the refusal |
| Stale TAO/USD feed | No effect on spending: the docs gate only **minting** on the feed (`TaoUsdPriceUnavailable` after 300 blocks). The relay never reads a price or converts TAO |
| Chain or RPC unavailable | Reads age past 10 minutes, so linked users at or below zero get the 402; users with a positive balance are untouched. Pending settlements wait; nothing is rebuilt until finality answers (§3.4 step 5) |
| Relay crash mid-settlement | Rows and signed bytes were stored first; restart resumes at step 3 or 4 |
| User pauses, re-points or empties the deposit | Their right, at any time. Seen on the next read; loss bounded by `E` |
| Subnet dissolved | The docs: its gamma is destroyed with nothing paid. No remedy on our side; say so in the client before a user mints |
| DB error | Gate fails closed (§2) |

---

## 6. Signed Requests and Validation

`PUT /me/gamma` is the only request a wallet key signs. `btauth/1` signs eight lines: version, scheme, method, request target, SHA-256 of the raw body, nonce, sender, receiver. It does not bind a chain or an explicit expiry, so those go in the body, which the hash covers.

| Must be bound | Where |
| --- | --- |
| Action | body `action`, must equal `gamma.link` |
| Method, route | `btauth/1` lines 3 and 4, rebuilt from the request as received |
| Body hash | line 5, over the raw bytes, computed before parsing |
| Resource ids | body `user_id` (must equal the device principal's), `netuid` (must equal ours), `hotkey` |
| Signer | line 7, must equal body `hotkey`; sr25519 only, fixed server-side, an `X-Bittensor-Crypto` header is rejected |
| Audience | line 8, must be our service hotkey; unbound requests rejected |
| Nonce | line 6; `(hotkey, nonce)` inserted into a unique table after the signature verifies; a second use is rejected |
| Expiry | body `expires_at`, at most 60 s ahead, and the `btauth/1` window (10 s past, 2 s future) |
| Chain | body `genesis`, must equal the genesis hash pinned in relay config |
| Schema version | the literal `btauth/1` and body `v` |

Rules:

- Unsigned, expired, replayed, wrong-audience and bad-signature requests all get the **same** 401 body. The body is parsed only after the signature verifies; unknown or duplicate fields are 400.
- **Every chain response is validated**: the node's genesis hash against the pinned one at connect; reads and events at finalized blocks only; a deposit's `netuid`, `hotkey` and `spender` against what was asked; amounts as exact non-negative decimals of at most 9 places, converted with integer arithmetic; anything else is an error, never a default.
- **Never spend more than metered** (§3.4 step 1), and every `gamma` ledger row traces to one finalized `GammaSpent` with our reference.
- The proxy key is relay env only (`ARBOS_GAMMA_SPEND_KEY`), like provider keys. `ARBOS_GAMMA_NETUID` unset turns the whole feature off: routes 404, gate and clients as today.
- Money routes are device-principal only and rate-limited through `rate_events`.
- Cost: the relay gains chain access (the `subxt` stack `arbos-wallet` already brings into the workspace). `arbos-wallet` itself stays device-only.

---

## 7. Considered, Sidelined

- **TAO cheques (#2932, #2935).** A cheque is a signed transfer that can bounce until someone cashes it, so crediting one before finality is free inference, and after finality it is just a transfer. They are off in the app (`chequesEnabled = false`). Gamma replaces them for paying for inference: the spender pulls, on chain, under a cap the user set. Cheques between people are a separate question this document does not touch.
- **TAO transfer to a relay address.** Needed a spot rate and custody of received TAO. Gamma is already dollar-valued and the relay holds nothing.
- **Card top-up (Stripe), gifting, sponsored rooms, a Bittensor upstream provider.** Not part of this revision; the text is in `fc6d71977`. Gamma cannot be gifted at all: it does not move between coldkeys. Users without TAO stay on the grant (decision 7).

---

## 8. Not Specified by the Bittensor Docs — Confirm on a Test Network Before Any Code

1. Whether gamma is enabled today on mainnet or testnet, and on which subnets. Not checked on chain.
2. The field layout of `GammaSpent` and `GammaSpendSkipped`, and the SCALE shape `GammaRuntimeApi.get_gamma_deposit` returns.
3. Which failures reject the whole `spend_gamma` call rather than skip one entry. Per-entry skipping is described; whole-call atomicity is not stated.
4. The fee per call and per entry, and any minimum spend (the 0.1 gamma minimum is stated for minting only). `E` and the tick are placeholders until this is measured.
5. What exactly the `GammaSpend` proxy type allows, and that the proxied call emits the same events.
6. `set-service`: the spender parameter, whether the hotkey must be registered, URL limits, what readers do when the document is unreachable.
7. Whether a hard-derived key (`//arbos//gamma`) can be named as the hotkey in `btcli gamma mint` and regenerated by standard tools.
8. Whether the local subtensor image the wallet tests use has gamma at all.
9. `btauth/1` is specified between hotkeys of validators, miners and service customers; using it for a one-time link rather than per request is our choice, not a documented pattern.

---

## 9. Phases

Each ships alone. Nothing writes to a public chain, and no service is published, without the owner's go-ahead. `ARBOS_GAMMA_NETUID` unset is the rollback for all of them.

- **Phase 0 — Ledger hardening (§2).** No visible change.
  - *Verify:* property test: any interleaving of grants, debits and repeated `idem` rows gives `SUM(micros)` equal to the model; a forced DB error refuses a metered call and allows a `paircode` one.
- **Phase 1 — Read-only: show a deposit's `spendable_gamma`.** The relay reads one deposit at a finalized block, behind an admin-only route. No link, no key, no spend.
  - *Verify:* for a test-network deposit the value equals `btcli query gamma-deposit --json`; recorded responses with a wrong netuid, hotkey, genesis or malformed amount are each rejected. This phase also answers §8 items 1, 2 and 8.
- **Phase 2 — Link and display (§3.2, §6).** Needs #2932.
  - *Verify:* one test per row of the §6 table (change it, expect the generic 401), plus replay, expiry and unknown field; iPhone and Mac show the same spendable figure as Phase 1.
- **Phase 3 — Settle on a local or test network (§3.3–3.4, §5).**
  - *Verify:* kill the relay before submit, after submit and before finality: exactly one `GammaSpent` per reference each time; each skip reason ends in the §5 state; property test that `Σ units ≤ Σ metered debits`; `Σ gamma rows = Σ GammaSpent` read back from the chain.
- **Phase 4 — Mainnet.** The owner publishes the service and creates the proxy (§3.5); `E` stays at 1 gamma.
  - *Verify:* one real deposit, one day, ledger and chain reconcile to the unit.

Minting inside the app is not planned here; until asked for, a user mints with `btcli` from the same phrase.

---

## 10. Decisions for const

1. **Which subnet's gamma?** Is there an Arbos subnet (netuid), who owns it, and is gamma enabled on it? *Default: none possible. Only Phase 0 proceeds until this is answered.*
2. **Who is repaid?** Spent gamma pays the spender nothing; the value goes to that subnet's alpha holders while upstream bills stay in dollars. *Default: proceed only if that is the intent.*
3. **Spender.** *Default: the subnet owner's service names a multisig spender; the relay holds only a `GammaSpend` proxy key.*
4. **Arrears or ahead.** The docs advise services to spend before serving, since a user can pause a deposit at any moment. Spending ahead would hold users' unspent gamma on our ledger with no way to return it. *Default: arrears, exposure `E` = 1 gamma per account.*
5. **Deposit hotkey.** *Default: derived from the wallet phrase (`//arbos//gamma`), not a second secret to back up.*
6. **Welcome grant.** It is spent before gamma and is $100 today. *Default: leave it for the beta; revisit at Phase 4.*
7. **Users with no TAO.** *Default: grant and admin top-up only; card top-up stays shelved.*
8. **Markup.** *Default: `0.0`.*
9. **Minting in the app.** *Default: no; `btcli` until asked.*
