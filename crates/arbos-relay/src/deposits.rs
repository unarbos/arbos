//! Paying for credits with TAO or subnet alpha: a deposit to the treasury becomes a ledger row.
//!
//! The relay holds no key. It watches finalized blocks for money arriving at one address
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
//! Unset `ARBOS_DEPOSIT_ADDRESS` and none of this runs; `GET /me/credits/deposit` is then 404.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_wallet::chain::{Chain, Deposit};

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

pub const DEPOSITS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS chain_deposits (
  block_hash TEXT NOT NULL, event_index INTEGER NOT NULL, block INTEGER NOT NULL,
  from_address TEXT NOT NULL, netuid INTEGER, rao INTEGER NOT NULL, valued_rao INTEGER NOT NULL,
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

    /// One finalized block, read: its deposits stored and the cursor moved, together.
    fn record_block(&self, number: u64, hash: &str, deposits: &[Deposit]) -> anyhow::Result<()> {
        self.with(|c| {
            let tx =
                rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
            for d in deposits {
                tx.execute(
                    "INSERT OR IGNORE INTO chain_deposits
                       (block_hash, event_index, block, from_address, netuid, rao, valued_rao, seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![hash, d.index, number as i64, d.from, d.netuid, d.rao as i64, d.valued_rao as i64, now_ms()],
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

    /// Is any stored deposit waiting only for a price?
    fn deposits_to_credit(&self) -> anyhow::Result<bool> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM chain_deposits d WHERE d.credited_at IS NULL AND {PAYEE} IS NOT NULL)"),
                [],
                |r| r.get(0),
            )
        })
    }

    /// Credit every uncredited deposit whose sender is linked, at `tao_usd`. Each is marked and
    /// its ledger row written in one transaction; the mark is what makes it once only.
    fn credit_deposits(&self, tao_usd: f64) -> anyhow::Result<usize> {
        self.with(|c| {
            let tx =
                rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
            let due: Vec<(String, i64, i64, Option<i64>, String)> = tx
                .prepare(&format!(
                    "SELECT d.block_hash, d.event_index, d.valued_rao, d.netuid, {PAYEE}
                     FROM chain_deposits d WHERE d.credited_at IS NULL AND {PAYEE} IS NOT NULL"
                ))?
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (hash, index, valued_rao, netuid, user) in &due {
                let tao = *valued_rao as f64 / 1e9;
                let usd = tao * tao_usd;
                let marked = tx.execute(
                    "UPDATE chain_deposits SET user_id = ?3, usd = ?4, tao_usd = ?5, credited_at = ?6
                     WHERE block_hash = ?1 AND event_index = ?2 AND credited_at IS NULL",
                    params![hash, index, user, usd, tao_usd, now_ms()],
                )?;
                // A deposit valued at nothing is settled with no ledger row.
                if marked == 1 && usd > 0.0 {
                    let note = match netuid {
                        None => format!("{tao} TAO at ${tao_usd}"),
                        Some(n) => format!("subnet {n} alpha worth {tao} TAO at ${tao_usd}"),
                    };
                    tx.execute(
                        "INSERT INTO credit_ledger(user_id, at, kind, usd, ref, note, actor)
                         VALUES (?1, ?2, 'deposit', ?3, ?4, ?5, 'chain')",
                        params![user, now_ms(), usd, format!("{hash}:{index}"), note],
                    )?;
                }
            }
            tx.commit()?;
            Ok(due.len())
        })
    }

    /// A user's deposits, newest first: those credited to them, and those still waiting from
    /// the address they have linked now.
    fn deposits_of(&self, user_id: &str, linked: Option<&str>) -> anyhow::Result<Vec<Value>> {
        self.with(|c| {
            c.prepare(
                "SELECT block, netuid, rao, valued_rao, usd, credited_at FROM chain_deposits
                 WHERE user_id = ?1 OR (credited_at IS NULL AND from_address = ?2)
                 ORDER BY block DESC, event_index DESC LIMIT 50",
            )?
            .query_map(params![user_id, linked], |r| {
                Ok(json!({
                    "block": r.get::<_, i64>(0)?, "netuid": r.get::<_, Option<i64>>(1)?,
                    "rao": r.get::<_, i64>(2)?, "valued_rao": r.get::<_, i64>(3)?,
                    "usd": r.get::<_, Option<f64>>(4)?, "credited_at": r.get::<_, Option<i64>>(5)?,
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

/// Read the finalized blocks not read yet, then credit what can be credited.
async fn tick(state: &AppState, chain: &Chain, treasury: &str) -> anyhow::Result<()> {
    let head = chain.finalized().await?;
    // The first run starts at the head: money sent before the watcher existed is not looked for.
    let next = state.db.deposit_cursor()?.map_or(head, |c| c + 1);
    for number in next..=head.min(next.saturating_add(BLOCKS_PER_TICK)) {
        let (hash, deposits) = chain.deposits(number, treasury).await?;
        for d in &deposits {
            tracing::info!(block = number, from = %d.from, netuid = ?d.netuid, rao = d.rao, valued_rao = d.valued_rao, "deposit seen");
        }
        state.db.record_block(number, &hash, &deposits)?;
    }
    if state.db.deposits_to_credit()? {
        let credited = state.db.credit_deposits(tao_usd(&state.http).await?)?;
        tracing::info!(credited, "deposits credited");
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
                match Chain::connect(&state.cfg.chain_rpc).await {
                    Ok(c) => chain = Some(c),
                    Err(error) => {
                        tracing::warn!(%error, "deposits: chain node");
                        continue;
                    }
                }
            }
            if let Some(c) = &chain {
                // Any failure leaves the cursor where it was; the next tick reads the block again.
                if let Err(error) = tick(&state, c, &treasury).await {
                    tracing::warn!(%error, "deposits");
                }
            }
        }
    });
}

/// `GET /me/credits/deposit` → `{address, linked, deposits}`: where to send TAO or alpha, the
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

    fn deposit(index: u32, netuid: Option<u16>, rao: u64, valued_rao: u64) -> Deposit {
        Deposit {
            index,
            from: ALICE.to_owned(),
            netuid,
            rao,
            valued_rao,
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

    /// Live: one tick against Finney stores the head block and moves the cursor.
    /// `ARBOS_CHAIN_RPC=https://archive.sub.latent.to cargo test -p arbos-relay live_ -- --ignored`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_a_tick_reads_the_head_and_a_price_is_sane() {
        let state = test_state(AuthMode::Github);
        let chain = Chain::connect(&std::env::var("ARBOS_CHAIN_RPC").expect("ARBOS_CHAIN_RPC"))
            .await
            .unwrap();
        tick(&state, &chain, ALICE).await.unwrap();
        let first = state.db.deposit_cursor().unwrap().expect("a cursor");
        tokio::time::sleep(PERIOD * 2).await;
        tick(&state, &chain, ALICE).await.unwrap();
        assert!(state.db.deposit_cursor().unwrap().unwrap() > first);
        assert!(SANE_TAO_USD.contains(&tao_usd(&state.http).await.unwrap()));
    }

    #[test]
    fn a_deposit_is_stored_once_and_credited_once_to_whoever_linked_its_sender_first() {
        let state = test_state(AuthMode::Github);
        let db = &state.db;
        assert_eq!(db.deposit_cursor().unwrap(), None);
        // 2 TAO, and alpha the chain called 1 TAO but an hour-old price halves.
        let block = [
            deposit(3, None, 2_000_000_000, 2_000_000_000),
            deposit(7, Some(28), 1_000_000_000, 500_000_000),
        ];
        db.record_block(10, "0xaa", &block).unwrap();
        db.record_block(10, "0xaa", &block).unwrap();
        assert_eq!(db.deposit_cursor().unwrap(), Some(10));

        // Nobody has linked the sender: nothing to credit, nothing credited.
        assert!(!db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0).unwrap(), 0);
        assert_eq!(db.deposits_of("gh_1", Some(ALICE)).unwrap().len(), 2);

        db.set_wallet("gh_1", Some(ALICE)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        db.set_wallet("gh_2", Some(ALICE)).unwrap();
        assert!(db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(200.0).unwrap(), 2);
        assert_eq!(balance(&state, "gh_1"), 2.5 * 200.0);
        assert_eq!(balance(&state, "gh_2"), 0.0);

        // Again, at another price, and after the block is read again: nothing moves.
        db.record_block(10, "0xaa", &block).unwrap();
        assert!(!db.deposits_to_credit().unwrap());
        assert_eq!(db.credit_deposits(900.0).unwrap(), 0);
        assert_eq!(balance(&state, "gh_1"), 500.0);
        let mine = db.deposits_of("gh_1", None).unwrap();
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0]["usd"], 100.0);
        assert!(db.deposits_of("gh_2", Some(ALICE)).unwrap().is_empty());
    }
}
