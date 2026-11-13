# Spec: Inference Payments — Card In, One Ledger, Bittensor Under the Hood

Status: **Proposed design** (2026-11-11). Nothing here is implemented; §8 lists the owner decisions that gate coding.
Companion docs: [`docs/self-host.md`](self-host.md), `docs/box-scaling.md` (#2957), `docs/matrix-groups-e2ee-spec.md` (#2958).

---

## 0. Goal & Core Decisions

**Grandma pays with a card and never learns what a subnet is.** Bittensor makes the default models cheap; it is not something she sees.

1. **Keep the one ledger and the one cap.** `credit_ledger` and the three-layer gate in `crates/arbos-relay/src/credits.rs` already meter every spend source. Payments are a new way for money to come *in*, not a new accounting system.
2. **Prepaid credit, bought on the web with a card.** Stripe Checkout (hosted page) → signed webhook → one `purchase` ledger row. No card data touches the relay; no new client code is needed to sell.
3. **Bittensor is an upstream, not a feature.** A Bittensor-served OpenAI-compatible provider becomes one more arm in `proxy::forward`, and its models become the defaults. Cheaper calls are the whole user-visible effect.
4. **Social is "who pays", decided in one function.** Gifting and sponsoring a room both reduce to `credits::payer()` and one new ledger kind.
5. **TAO is an optional, later top-up rail** for people who already hold it. It is never required and never shown by default.

---

## 1. Starting Point: What Exists on `main`

| Piece | Where | State |
| --- | --- | --- |
| Metering proxy — boxes hold no provider key | `crates/arbos-relay/src/proxy.rs` (`/proxy/v1/*`, `/proxy/anthropic/*`) | Shipping. Upstreams: OpenRouter, OpenAI direct, Anthropic direct |
| Per-user ledger | `credit_ledger(user_id, at, kind, usd REAL, ref, note, actor)` in `db_accounts.rs` | Shipping. Kinds: `grant`, `inference`, `voice`, `machine` |
| Spend gate | `credits::check` — kill switch → balance → one daily limit | Shipping |
| Payer resolution | `credits::payer()` — the principal's user, else the project owner | Shipping |
| Money in | Welcome grant (`ARBOS_DEFAULT_GRANT_USD`) and `PUT /admin/users/{id}/credits` | **Admin-only. No purchase path.** |
| Pricing | Upstream-reported `usage.cost`, else a substring-matched table (`proxy::price`) | **Zero margin** |
| Client UI | iPhone `CreditsPage` (`SettingsView.swift`), Mac `SettingsPage::Credits` | Balance + "Request a top-up" → feedback sheet |
| Site | `site/index.html` pricing cards | "$0 during the beta", Pro "Soon" |

Not on `main`: any payment processor, StoreKit, any Bittensor provider, any user-to-user transfer. The wallet stack (#2932, #2933, #2935) gives each user a TAO address and peer-to-peer cheques, but is deliberately device-only and unconnected to the ledger.

---

## 2. Ledger Hardening (Prerequisite — Ships Before Any Money Moves)

The ledger was built for grants. Four properties must hold before a card is charged against it:

| Today | Problem once real money is in | Change |
| --- | --- | --- |
| `usd REAL`, balance = `SUM(usd)` | Float drift on a money column | Add `micros INTEGER NOT NULL` (1 USD = 1,000,000). Backfill `round(usd * 1e6)`. Balance and caps read `SUM(micros)`. Keep writing `usd` for one release, then drop its readers |
| Rows carry no idempotency key | A retried webhook or debit double-counts | Add `idem TEXT`, with `CREATE UNIQUE INDEX … ON credit_ledger(idem) WHERE idem IS NOT NULL`. Purchases use the Stripe event id; debits use the proxy `req_id` |
| `credits::check` returns `Ok` on a DB error | A locked DB means free inference | Fail **closed** for metered accounts (503, retryable). Self-host (`paircode`) stays ungated |
| `charge` clamps an overshoot at the balance | The operator absorbs the tail of every final call | Debit the true cost and let the balance go at most one call negative; the next `check` refuses. The purchase that follows repays it |

New kinds: `purchase`, `refund`, `transfer_out`, `transfer_in`. Refunds and transfers are rows, never edits — the ledger stays append-only.

**Sequencing:** #2951 rewrites voice/vision settlement in `credits.rs` and `db_accounts.rs`. Land or decline it first; this section conflicts with it either way.

---

## 3. Money In: Card via Stripe Checkout

```
client ── POST /me/credits/checkout {pack} ──▶ relay ── create Checkout Session ──▶ Stripe
client ◀── {url} ─────────────────────────────  relay
client ── opens url in the system browser, pays ─────────────────────────────────▶ Stripe
relay  ◀── POST /stripe/webhook  checkout.session.completed (signed) ───────────── Stripe
relay  ── INSERT credit_ledger(kind='purchase', micros, idem=event.id) ; push balance to devices
```

- **`POST /me/credits/checkout`** — device principal only. Body is exactly `{ "pack": "p5" | "p10" | "p25" | "p50" }`; unknown fields and unknown packs are 400. The pack → price mapping lives on the relay; **the client never sends an amount.** The session carries `client_reference_id = user_id`. Rate-limited per user through the existing `rate_events` table.
- **`POST /stripe/webhook`** — unauthenticated route whose only credential is the `Stripe-Signature` header, verified against `STRIPE_WEBHOOK_SECRET` over the **raw** body, with the timestamp tolerance enforced. The amount credited comes from the verified event (`amount_total`, `currency == "usd"`, `payment_status == "paid"`), never from the request that started checkout. The unique `idem` index makes redelivery a no-op. Anything that fails validation gets a generic 400.
- **Refunds and disputes** — `charge.refunded` and `charge.dispute.created` write a negative `refund` row. A balance driven below zero simply refuses further spend.
- **Config** — `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_P5…` are relay env only, like the provider keys. Unset means the route returns 404 and the clients keep today's copy, so self-hosters and the beta are unaffected.
- **No new dependency is required** — two HTTPS calls with the relay's existing HTTP client and one HMAC-SHA256 verification.

**Clients.** The "Request a top-up" row on both `CreditsPage`s becomes **Add credit**, which opens the returned URL in the system browser; the balance updates live when the webhook lands. The site's Pro card links to the same flow.

**Why packs, not a subscription, first:** it is the smallest thing that matches the ledger as built (a positive row), has no proration, dunning or entitlement state, and is what the site's "usage credit" copy already describes. A subscription later is a monthly `grant` row driven by `invoice.paid` — the same webhook, one more event type.

**Sign-in is a dependency, not part of this spec.** Accounts are GitHub users today (`signin.rs`). Grandma has no GitHub account; card payments do not help her until Apple / email sign-in exists.

---

## 4. Pricing & Margin

- One knob: `ARBOS_INFERENCE_MARKUP` (default `0.0` = today's pass-through). The debit is `upstream_cost × (1 + markup)`, applied in `Meter::usd` so every spend source inherits it. The ledger row's `note` records the upstream cost for audit.
- Replace the substring-matched `proxy::price` table with exact ids loaded from the upstream's own `/models` pricing, cached. A model with no known price and no reported cost is **refused**, not guessed — today any id in OpenRouter's catalog is selectable, from Haiku to models 50× dearer.
- Machine time stays informational and outside the balance, as `credits.rs` has it, until `docs/box-scaling.md` Phase 2 makes boxes sleep (an always-on box billed hourly would drain every balance).

---

## 5. Bittensor Under the Hood

- **Provider arm.** Add one OpenAI-compatible upstream to the `match api` in `proxy::forward`, configured by `ARBOS_BITTENSOR_BASE` + `ARBOS_BITTENSOR_KEY`, selected by model-id prefix exactly the way `openai_goes_direct` routes `openai/*` today. The relay holds the key; boxes are unchanged.
- **Cost source.** Use the provider's reported usage cost when it sends one, else its published per-token rates (§4's exact-id table).
- **Defaults.** The three per-project slots (`talking_model`, `subagent_model`, `decision_model` in `api.rs` / `settings.rs`) default to Bittensor-served open models once they pass the eval gate in `evals/`. Claude and GPT stay one tap away in `ModelPickerView` at their real price. This is where "way cheaper" comes from, and the only place the user meets it — as a lower number.
- **Failover.** A provider 5xx or timeout retries once on the equivalent OpenRouter model at the same quoted price, so a miner outage is not a user-visible error.
- **Which provider** is owner decision 3. Requirements: OpenAI-compatible chat + streaming + tool calls, per-request usage reporting, and a stable key the relay can hold.

---

## 6. Social: Gifting and Sponsored Rooms

Both are small because the payer is already resolved in one place.

- **Gift credit.** `POST /me/credits/gift { "to": "<user_id>", "pack": … }` writes `transfer_out` and `transfer_in` in **one** SQLite transaction with a shared `idem`. Only purchased credit is giftable (granted beta credit is not), with a per-day cap, so a welcome grant cannot be farmed across accounts.
- **Sponsored room.** A group gains an optional `sponsor_user_id`. `credits::payer()` returns it for agent turns in that room, so one person can run an agent for a whole group. The sponsor sets it; only the sponsor or a room admin clears it; the sponsor's one daily limit bounds it.
- **"Gamma".** The term appears nowhere in this repository. If it names a specific token or allowance to be shared between users, it maps onto one of the two mechanisms above — see owner decision 5.

---

## 7. TAO Top-Up (Optional, Later)

For users who already hold TAO and linked an address (#2933):

- The relay publishes one deposit address. A **finalized** `Balances.transfer` to it from a user's *linked* address credits that user a `purchase` row at the spot rate when finality is observed, `idem` = block hash + extrinsic index.
- Cheques (#2932) are not used for this: they are bounceable until cashed, and a cheque shown as credit before finality is a free-inference bug.
- The relay only watches the chain. It holds a deposit address, never a user key — the wallet stays device-only.
- The surface lives inside Settings → Wallet and is invisible to anyone who never opened it.

---

## 8. Owner Decisions Needed Before Coding

1. **Margin.** Stay at zero markup, or set `ARBOS_INFERENCE_MARKUP` (and to what)?
2. **Packs and free tier.** Pack sizes, and what the welcome grant becomes once people can pay (today: $100).
3. **Bittensor provider.** Which OpenAI-compatible provider(s) the relay routes to, and which models become the defaults.
4. **iPhone purchase surface.** Selling consumable credit inside the iOS app is In-App Purchase territory under App Store rule 3.1.1, and external-purchase links are permitted only in some storefronts. Options: (a) web-only, with the iPhone showing balance and no purchase button; (b) an external link where the storefront allows it; (c) StoreKit consumables at Apple's commission. Recommended: (a) for the first release, since it needs no review risk and no client code.
5. **"Gamma".** What it refers to, and whether sharing it means gifting (§6) or sponsoring (§6).
6. **Sign-in.** Whether Apple / email sign-in lands before payments are announced.

---

## 9. Phased Rollout

Each phase is flagged by its env being unset, so rollback is removing the variable.

- **Phase 0 — Ledger hardening (§2).** No user-visible change.
  - *Gate:* property test — any interleaving of grants, debits and retried `idem` rows yields `SUM(micros)` equal to the model; a forced DB error refuses a metered call and allows a `paircode` one.
- **Phase 1 — Card top-up on the web (§3).** Stripe test mode on a staging relay, then live.
  - *Gate:* a replayed webhook credits once; a tampered body, a stale timestamp and a wrong secret are each rejected; a refund drives the balance down and the next call returns 402; `POST /me/credits/checkout` with an unknown field or pack is 400.
- **Phase 2 — Pricing table and markup (§4).**
  - *Gate:* an unpriced model id is refused; the sum of a day's ledger debits equals the day's upstream invoices × (1 + markup) within rounding.
- **Phase 3 — Bittensor upstream and defaults (§5).**
  - *Gate:* the existing `evals/` suite on the candidate default models meets the bar the current defaults set; killing the provider mid-stream fails over with no user-visible error.
- **Phase 4 — Gifting and sponsored rooms (§6).**
  - *Gate:* a gift is atomic under a crash between the two rows; granted credit cannot be gifted; a sponsored room debits the sponsor and stops at the sponsor's limit.
- **Phase 5 — TAO top-up (§7).**
  - *Gate:* a transfer credits only after finality and only from a linked address; a reorged block credits nothing.

---

## 10. Security Invariants

- The relay never sees or stores card data; Stripe's hosted page does.
- Every credit to a balance is traceable to one verified external event (`idem`) or one named admin (`actor`).
- The client never names an amount. Packs are relay-side constants; webhook amounts come from the signed event.
- Money routes are device-principal only, reject unknown fields, are rate-limited, and return generic errors.
- The single static `ARBOS_ADMIN_TOKEN` can mint credit through `PUT /admin/users/{id}/credits`. Before live payments, admin grants need a per-admin identity in `actor` and an alert on any grant above a threshold.
- The spend gate fails closed for metered accounts.
