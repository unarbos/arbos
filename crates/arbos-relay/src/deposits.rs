//! Paying for credits with TAO: a transfer to the treasury becomes a ledger row.
//!
//! The relay holds no key. It watches finalized blocks for TAO arriving at one address
//! (`ARBOS_DEPOSIT_ADDRESS`, a cold wallet) and credits whoever proved they hold the address it
//! came from (`PUT /me/wallet`, `wallets.rs`):
//!
//!   1. each finalized block is read once, in order, from a saved cursor; what it paid the
//!      treasury is stored under (block hash, event index) in the same transaction that moves
//!      the cursor, so a crash or a restart neither skips a block nor stores a deposit twice;
//!   2. a stored deposit whose sender is linked to a user is credited: the deposit row is
//!      marked and the `deposit` ledger row written in one transaction, at TAO's USD price then.
//!
//! A deposit from an address nobody has linked waits, and is credited when its holder links it.
//! Two users who both proved the same address: the one who linked it first is paid.
//!
//! Only TAO counts. Stake (alpha) handed to the treasury is not a deposit, and neither is a
//! transfer made inside a stake move or outside an extrinsic (`chain.rs`). A sender that cannot
//! sign a link proof is never credited from here: an exchange's wallet, a multisig, a pure
//! proxy, a key that is not sr25519. Those are paid by hand, with the admin grant.
//!
//! The chain node is somebody else's, and a lying one could invent deposits. Nothing here
//! checks what it says against a second node. Two limits bound what a lie can cost:
//!
//!   - a deposit above `ARBOS_DEPOSIT_MAX_TAO` when it is seen is held for good: stored and
//!     shown, never credited from here, whatever the cap is later. A person checks it against
//!     the chain and grants it by hand;
//!   - no more than `ARBOS_DEPOSIT_DAILY_MAX_TAO` is credited from here in any 24 hours. A
//!     deposit that would pass it waits, and is credited when the day has room.
//!
//! So a lying node can cost at most the daily limit a day, in deposits no larger than the cap.
//!
//! Unset `ARBOS_DEPOSIT_ADDRESS` and none of this runs; `GET /me/credits/deposit` is then 404.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::chain::{Chain, Deposit};
use crate::signin::api_error;
use crate::state::AppState;

pub const DEPOSITS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS chain_deposits (
  block_hash TEXT NOT NULL, event_index INTEGER NOT NULL, block INTEGER NOT NULL,
  from_address TEXT NOT NULL, rao INTEGER NOT NULL, held INTEGER NOT NULL DEFAULT 0,
  seen_at INTEGER NOT NULL, user_id TEXT, usd REAL, tao_usd REAL, credited_at INTEGER,
  PRIMARY KEY (block_hash, event_index)
);
CREATE INDEX IF NOT EXISTS chain_deposits_pending ON chain_deposits(from_address) WHERE credited_at IS NULL;
CREATE INDEX IF NOT EXISTS chain_deposits_user ON chain_deposits(user_id);
CREATE TABLE IF NOT EXISTS chain_cursor (id INTEGER PRIMARY KEY CHECK (id = 1), block INTEGER NOT NULL);
"#;

/// Subtensor's block time.
const PERIOD: Duration = Duration::from_secs(12);
/// Blocks read per tick while catching up after downtime (a block is two or three requests).
const BLOCKS_PER_TICK: u64 = 100;
/// TAO's USD price. Read when a deposit is credited, which is seconds after its block unless
/// the relay was down: a deposit made during downtime is credited at the price on return.
const TAO_USD_URL: &str =
    "https://api.coingecko.com/api/v3/simple/price?ids=bittensor&vs_currencies=usd";
/// A price outside this is a broken feed, not a market: nothing is credited until it is sane.
const SANE_TAO_USD: std::ops::RangeInclusive<f64> = 1.0..=100_000.0;

/// The sender's linked user, when there is one: the earliest link wins.
const PAYEE: &str = "(SELECT w.user_id FROM user_wallets w WHERE w.address = d.from_address
                      ORDER BY w.linked_at, w.user_id LIMIT 1)";

impl crate::db::Db {
    fn deposit_cursor(&self) -> anyhow::Result<Option<u64>> {
        self.with(|c| {
            c.query_row("SELECT block FROM chain_cursor WHERE id = 1", [], |r| {
                r.get::<_, i64>(0)
            })
            .optional()
            .map(|b| b.map(|b| b as u64))
        })
    }

    /// One finalized block, read: its deposits stored and the cursor moved, together. A deposit
    /// above `max_rao` is stored held, and stays held.
    fn record_block(
        &self,
        number: u64,
        hash: &str,
        deposits: &[Deposit],
        max_rao: i64,
    ) -> anyhow::Result<()> {
        self.with(|c| {
            let tx =
                rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
            for d in deposits {
                tx.execute(
                    "INSERT OR IGNORE INTO chain_deposits
                       (block_hash, event_index, block, from_address, rao, held, seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5 > ?6, ?7)",
                    params![
                        hash,
                        d.index,
                        number as i64,
                        d.from,
                        d.rao as i64,
                        max_rao,
                        now_ms()
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO chain_cursor(id, block) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET block = excluded.block",
                params![number as i64],
            )?;
            tx.commit()
        })
    }

    /// Is any stored deposit waiting only for a price, or for the day to have room?
    fn deposits_to_credit(&self) -> anyhow::Result<bool> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM chain_deposits d WHERE d.credited_at IS NULL AND NOT d.held AND {PAYEE} IS NOT NULL)"),
                [],
                |r| r.get(0),
            )
        })
    }

    /// Credit every uncredited deposit that is not held and whose sender is linked, at
    /// `tao_usd`, oldest first. Each is marked and its ledger row written in one transaction;
    /// the mark is what makes it once only. One that would take what was credited in the last
    /// 24 hours above `daily_max_rao` is left waiting. Returns how many were credited and how
    /// many left.
    fn credit_deposits(&self, tao_usd: f64, daily_max_rao: i64) -> anyhow::Result<(usize, usize)> {
        self.with(|c| {
            let tx =
                rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
            // A row settled by hand has no `usd` and is not counted.
            let mut day: i64 = tx.query_row(
                "SELECT COALESCE(SUM(rao), 0) FROM chain_deposits
                 WHERE credited_at > ?1 AND usd IS NOT NULL",
                [now_ms() - 24 * 60 * 60 * 1000],
                |r| r.get(0),
            )?;
            let due: Vec<(String, i64, i64, String)> = tx
                .prepare(&format!(
                    "SELECT d.block_hash, d.event_index, d.rao, {PAYEE}
                     FROM chain_deposits d
                     WHERE d.credited_at IS NULL AND NOT d.held AND {PAYEE} IS NOT NULL
                     ORDER BY d.block, d.event_index"
                ))?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut credited = 0;
            for (hash, index, rao, user) in &due {
                if day.saturating_add(*rao) > daily_max_rao {
                    continue;
                }
                let tao = *rao as f64 / 1e9;
                let usd = tao * tao_usd;
                let marked = tx.execute(
                    "UPDATE chain_deposits SET user_id = ?3, usd = ?4, tao_usd = ?5, credited_at = ?6
                     WHERE block_hash = ?1 AND event_index = ?2 AND credited_at IS NULL",
                    params![hash, index, user, usd, tao_usd, now_ms()],
                )?;
                if marked == 1 {
                    let note = format!("{tao} TAO at ${tao_usd}");
                    tx.execute(
                        "INSERT INTO credit_ledger(user_id, at, kind, usd, ref, note, actor)
                         VALUES (?1, ?2, 'deposit', ?3, ?4, ?5, 'chain')",
                        params![user, now_ms(), usd, format!("{hash}:{index}"), note],
                    )?;
                    day += rao;
                    credited += 1;
                }
            }
            tx.commit()?;
            Ok((credited, due.len() - credited))
        })
    }

    /// A user's deposits, newest first: those credited to them, and those still waiting from
    /// the address they have linked now.
    fn deposits_of(&self, user_id: &str, linked: Option<&str>) -> anyhow::Result<Vec<Value>> {
        self.with(|c| {
            c.prepare(
                "SELECT block, rao, usd, credited_at, credited_at IS NULL AND held
                 FROM chain_deposits
                 WHERE user_id = ?1 OR (credited_at IS NULL AND from_address = ?2)
                 ORDER BY block DESC, event_index DESC LIMIT 50",
            )?
            .query_map(params![user_id, linked], |r| {
                Ok(json!({
                    "block": r.get::<_, i64>(0)?, "rao": r.get::<_, i64>(1)?,
                    "usd": r.get::<_, Option<f64>>(2)?, "credited_at": r.get::<_, Option<i64>>(3)?,
                    "held": r.get::<_, bool>(4)?,
                }))
            })?
            .collect()
        })
    }
}

async fn tao_usd(http: &reqwest::Client) -> anyhow::Result<f64> {
    let v: Value = http
        .get(TAO_USD_URL)
        // CoinGecko answers 403 to a request with no User-Agent.
        .header("user-agent", "Arbos")
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    match v["bittensor"]["usd"].as_f64() {
        Some(p) if SANE_TAO_USD.contains(&p) => Ok(p),
        _ => anyhow::bail!("no sane TAO price in the reply"),
    }
}

fn rao(tao: f64) -> i64 {
    (tao * 1e9) as i64
}

/// Read the finalized blocks not read yet. Any failure leaves the cursor where it was; the
/// next tick reads the block again.
async fn read_blocks(state: &AppState, chain: &Chain, treasury: &str) -> anyhow::Result<()> {
    // A load-balanced endpoint may answer the head and the block from different backends.
    let head = chain.finalized().await?.saturating_sub(2);
    let max_rao = rao(state.cfg.deposit_max_tao);
    // The first run starts at the head: money sent before the watcher existed is not looked for.
    let next = state.db.deposit_cursor()?.map_or(head, |c| c + 1);
    for number in next..=head.min(next.saturating_add(BLOCKS_PER_TICK)) {
        let (hash, deposits) = chain.deposits(number, treasury).await?;
        for d in &deposits {
            tracing::info!(block = number, from = %d.from, rao = d.rao, "deposit seen");
            if d.rao as i64 > max_rao {
                tracing::warn!(block = number, from = %d.from, rao = d.rao, "deposit held for review");
            }
        }
        state.db.record_block(number, &hash, &deposits, max_rao)?;
    }
    Ok(())
}

/// Credit what can be credited.
async fn credit(state: &AppState) -> anyhow::Result<()> {
    if state.db.deposits_to_credit()? {
        let daily_max = rao(state.cfg.deposit_daily_max_tao);
        let (credited, waiting) = state
            .db
            .credit_deposits(tao_usd(&state.http).await?, daily_max)?;
        tracing::info!(credited, "deposits credited");
        if waiting > 0 {
            tracing::warn!(waiting, "deposits wait: the 24-hour limit is reached");
        }
    }
    Ok(())
}

pub async fn spawn_watcher(state: AppState) {
    let Some(treasury) = state.cfg.deposit_address.clone() else {
        return;
    };
    if !crate::credits::metered(&state) {
        return;
    }
    tokio::spawn(async move {
        let mut tick_at = tokio::time::interval(PERIOD);
        tick_at.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut chain = None;
        loop {
            tick_at.tick().await;
            if chain.is_none() {
                match Chain::connect(&state.cfg.chain_rpc, state.http.clone()).await {
                    Ok(c) => chain = Some(c),
                    Err(error) => tracing::warn!(%error, "deposits: chain node"),
                }
            }
            if let Some(c) = &chain {
                if let Err(error) = read_blocks(&state, c, &treasury).await {
                    tracing::warn!(%error, "deposits: reading blocks");
                }
            }
            // Whatever the chain node did: a deposit already stored is credited all the same.
            if let Err(error) = credit(&state).await {
                tracing::warn!(%error, "deposits: crediting");
            }
        }
    });
}

/// `GET /me/credits/deposit` → `{address, linked, deposits}`: where to send TAO, the
/// address this user is credited from, and their deposits. 404 when the relay takes none.
pub async fn info(State(state): State<AppState>, principal: Principal) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    let Some(address) = &state.cfg.deposit_address else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "deposits are not enabled",
        );
    };
    let found = state.db.wallet(user_id).and_then(|linked| {
        let deposits = state.db.deposits_of(user_id, linked.as_deref())?;
        Ok((linked, deposits))
    });
    match found {
        Ok((linked, deposits)) => {
            Json(json!({ "address": address, "linked": linked, "deposits": deposits }))
                .into_response()
        }
        Err(error) => {
            tracing::error!(%error, "deposit info");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db",
                "something went wrong",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{test_support::test_state, AuthMode};

    const ALICE: &str = "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV";

    fn deposit(index: u32, rao: u64) -> Deposit {
        Deposit {
            index,
            from: ALICE.to_owned(),
            rao,
        }
    }

    fn balance(state: &AppState, user: &str) -> f64 {
        let day = crate::credits::start_of_utc_day_ms();
        state
            .db
            .debit(user, 0.0, "check", None, day)
            .unwrap()
            .unwrap()
    }

    /// Live: one read of Finney stores the head block and moves the cursor.
    /// `ARBOS_CHAIN_RPC=https://archive.sub.latent.to cargo test -p arbos-relay live_ -- --ignored`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_a_tick_reads_the_head_and_a_price_is_sane() {
        let state = test_state(AuthMode::Github);
        let chain = Chain::connect(
            &std::env::var("ARBOS_CHAIN_RPC").expect("ARBOS_CHAIN_RPC"),
            state.http.clone(),
        )
        .await
        .unwrap();
        read_blocks(&state, &chain, ALICE).await.unwrap();
        let first = state.db.deposit_cursor().unwrap().expect("a cursor");
        tokio::time::sleep(PERIOD * 2).await;
        read_blocks(&state, &chain, ALICE).await.unwrap();
        assert!(state.db.deposit_cursor().unwrap().unwrap() > first);
        assert!(SANE_TAO_USD.contains(&tao_usd(&state.http).await.unwrap()));
    }

    const MAX: i64 = 25_000_000_000;
    const DAY: i64 = 100_000_000_000;

    #[test]
    fn a_deposit_is_stored_once_and_credited_once_to_whoever_linked_its_sender_first() {
        let state = test_state(AuthMode::Github);
        let db = &state.db;
        assert_eq!(db.deposit_cursor().unwrap(), None);
        let block = [deposit(3, 2_000_000_000), deposit(7, 500_000_000)];
        db.record_block(10, "0xaa", &block, MAX).unwrap();
        db.record_block(10, "0xaa", &block, MAX).unwrap();
        assert_eq!(db.deposit_cursor().unwrap(), Some(10));

        // Nobody has linked the sender: nothing to credit, nothing credited.
        assert!(!db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0, DAY).unwrap(), (0, 0));
        assert_eq!(db.deposits_of("gh_1", Some(ALICE)).unwrap().len(), 2);

        db.set_wallet("gh_1", Some(ALICE)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        db.set_wallet("gh_2", Some(ALICE)).unwrap();
        assert!(db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0, DAY).unwrap(), (2, 0));
        assert_eq!(balance(&state, "gh_1"), 2.5 * 200.0);
        assert_eq!(balance(&state, "gh_2"), 0.0);

        // Again, at another price, and after the block is read again: nothing moves.
        db.record_block(10, "0xaa", &block, MAX).unwrap();
        assert!(!db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(900.0, DAY).unwrap(), (0, 0));
        assert_eq!(balance(&state, "gh_1"), 500.0);
        let mine = db.deposits_of("gh_1", None).unwrap();
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0]["usd"], 100.0);
        assert_eq!(mine[0]["held"], false);
        assert!(db.deposits_of("gh_2", Some(ALICE)).unwrap().is_empty());
    }

    #[test]
    fn a_deposit_above_the_cap_stays_held_and_one_at_it_is_credited() {
        let state = test_state(AuthMode::Github);
        let db = &state.db;
        db.set_wallet("gh_1", Some(ALICE)).unwrap();
        db.record_block(10, "0xaa", &[deposit(1, MAX as u64 + 1)], MAX)
            .unwrap();
        assert!(!db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0, DAY).unwrap(), (0, 0));

        // The cap is raised, and the block read again under it: the row is still held.
        db.record_block(10, "0xaa", &[deposit(1, MAX as u64 + 1)], MAX * 2)
            .unwrap();
        db.record_block(11, "0xbb", &[deposit(1, MAX as u64)], MAX * 2)
            .unwrap();
        assert!(db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0, DAY).unwrap(), (1, 0));
        assert_eq!(balance(&state, "gh_1"), 25.0 * 200.0);
        let mine = db.deposits_of("gh_1", Some(ALICE)).unwrap();
        assert_eq!(
            (&mine[0]["held"], &mine[0]["usd"]),
            (&json!(false), &json!(5000.0))
        );
        assert_eq!(
            (&mine[1]["held"], &mine[1]["usd"]),
            (&json!(true), &Value::Null)
        );
    }

    #[test]
    fn a_deposit_settled_by_hand_is_not_credited_when_its_sender_links() {
        let state = test_state(AuthMode::Github);
        let db = &state.db;
        db.record_block(10, "0xaa", &[deposit(1, 1_000_000_000)], MAX)
            .unwrap();
        // The operator's step after a manual grant (docs/self-host.md).
        db.with(|c| {
            c.execute(
                "UPDATE chain_deposits SET credited_at = 1, user_id = 'gh_1'
                 WHERE block_hash = '0xaa' AND event_index = 1",
                [],
            )
        })
        .unwrap();
        db.set_wallet("gh_1", Some(ALICE)).unwrap();
        assert!(!db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0, DAY).unwrap(), (0, 0));
        assert_eq!(balance(&state, "gh_1"), 0.0);
    }

    #[test]
    fn no_more_than_the_daily_limit_is_credited_in_24_hours() {
        let state = test_state(AuthMode::Github);
        let db = &state.db;
        db.set_wallet("gh_1", Some(ALICE)).unwrap();
        let tao = 1_000_000_000;
        let block = [
            deposit(1, 20 * tao),
            deposit(2, 20 * tao),
            deposit(3, 5 * tao),
        ];
        db.record_block(10, "0xaa", &block, MAX).unwrap();
        // 20, then 20 would make 40 and waits, then 5 makes 25.
        assert_eq!(db.credit_deposits(200.0, 30 * tao as i64).unwrap(), (2, 1));
        assert_eq!(balance(&state, "gh_1"), 25.0 * 200.0);
        // Still waiting, not held, and what was credited counts on the next pass.
        assert!(db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0, 30 * tao as i64).unwrap(), (0, 1));
        assert_eq!(
            db.deposits_of("gh_1", Some(ALICE)).unwrap()[1]["held"],
            false
        );
        // A day later the earlier credits have aged out.
        db.with(|c| {
            c.execute(
                "UPDATE chain_deposits SET credited_at = 1 WHERE credited_at IS NOT NULL",
                [],
            )
        })
        .unwrap();
        assert_eq!(db.credit_deposits(200.0, 30 * tao as i64).unwrap(), (1, 0));
        assert_eq!(balance(&state, "gh_1"), 45.0 * 200.0);
    }
}
