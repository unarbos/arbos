//! Reading the Bittensor chain for the deposit watcher (`deposits.rs`): finalized blocks only,
//! over plain JSON-RPC on HTTPS. Nothing here signs or holds a key. The wallet crate has a
//! chain client of its own, but it is the apps' alone (CI checks its dependents), so the relay
//! reads the chain itself.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::value::RawValue;
use subxt::backend::LegacyBackend;
use subxt::config::SubstrateConfig;
use subxt::utils::AccountId32;
use subxt::OnlineClient;

type Api = OnlineClient<SubstrateConfig>;

struct HttpRpc {
    url: String,
    http: reqwest::Client,
}

impl subxt::rpcs::RpcClientT for HttpRpc {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<RawValue>>,
    ) -> subxt::rpcs::client::RawRpcFuture<'a, Box<RawValue>> {
        Box::pin(async move {
            let client_err = |e: String| subxt::rpcs::Error::Client(e.into());
            let body = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":{},"params":{}}}"#,
                serde_json::Value::from(method),
                params.as_deref().map(RawValue::get).unwrap_or("[]")
            );
            let text = self
                .http
                .post(&self.url)
                .header("content-type", "application/json")
                .timeout(Duration::from_secs(30))
                .body(body)
                .send()
                .await
                .map_err(|e| client_err(e.to_string()))?
                .text()
                .await
                .map_err(|e| client_err(e.to_string()))?;
            // `result` may legitimately be null (absent storage): check `error` first.
            #[derive(Deserialize)]
            struct Failure {
                error: Option<serde_json::Value>,
            }
            #[derive(Deserialize)]
            struct Success {
                result: Box<RawValue>,
            }
            let de = subxt::rpcs::Error::Deserialization;
            if let Some(e) = serde_json::from_str::<Failure>(&text).map_err(de)?.error {
                return Err(client_err(format!("{method}: {e}")));
            }
            Ok(serde_json::from_str::<Success>(&text).map_err(de)?.result)
        })
    }

    fn subscribe_raw<'a>(
        &'a self,
        _sub: &'a str,
        _params: Option<Box<RawValue>>,
        _unsub: &'a str,
    ) -> subxt::rpcs::client::RawRpcFuture<'a, subxt::rpcs::client::RawRpcSubscription> {
        Box::pin(async {
            Err(subxt::rpcs::Error::Client(
                "no subscriptions over HTTP".into(),
            ))
        })
    }
}

fn ss58(account: &[u8; 32]) -> String {
    AccountId32(*account).to_string()
}

/// Money that arrived at a treasury address in one finalized block, read from the block's
/// events, so a transfer made through a proxy, a batch or a multisig is seen like any other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deposit {
    /// The event's position in its block: with the block hash, the deposit's identity.
    pub index: u32,
    /// Who paid: the account the TAO left, or the coldkey the stake left.
    pub from: String,
    /// `None` for TAO; the subnet whose alpha stake was handed over otherwise.
    pub netuid: Option<u16>,
    /// What the chain says moved, in rao of TAO (for alpha: its TAO value at the block's price).
    pub rao: u64,
    /// What it is worth to the treasury, in rao: `rao` for TAO, [`alpha_value`] for alpha.
    pub valued_rao: u64,
}

/// Blocks in the hour an alpha deposit's price is checked against.
const PRICE_LOOKBACK: u64 = 300;

/// Alpha handed over inside one subnet is valued by the chain at that block's pool price, which
/// whoever sends it can push up for a block with a trade of their own. So it is worth the lower
/// of that price and the price an hour earlier: holding a pool up for an hour means selling to
/// everyone who notices. Prices are rao of TAO per alpha.
fn alpha_value(rao: u64, price_at: u64, price_before: u64) -> u64 {
    if price_at == 0 {
        return 0;
    }
    (rao as u128 * price_before.min(price_at) as u128 / price_at as u128) as u64
}

/// A chain node by URL.
#[derive(Clone)]
pub struct Chain(Api);

impl Chain {
    pub async fn connect(url: &str, http: reqwest::Client) -> anyhow::Result<Self> {
        let rpc = subxt::rpcs::RpcClient::new(HttpRpc {
            url: url.to_owned(),
            http,
        });
        let backend = LegacyBackend::builder().build(rpc);
        Ok(Self(Api::from_backend(Arc::new(backend)).await?))
    }

    /// The newest finalized block. Nothing above it is ever read.
    pub async fn finalized(&self) -> anyhow::Result<u64> {
        Ok(self.0.at_current_block().await?.block_number())
    }

    async fn alpha_price(&self, number: u64, netuid: u16) -> anyhow::Result<u64> {
        let at = self.0.at_block(number).await?;
        let raw = at
            .runtime_apis()
            .call_raw(
                "SwapRuntimeApi_current_alpha_price",
                Some(&netuid.to_le_bytes()),
            )
            .await?;
        let raw: [u8; 8] = raw[..].try_into()?;
        Ok(u64::from_le_bytes(raw))
    }

    /// Block `number`'s hash and every payment in it: `(to, same_subnet_alpha, deposit)`, with
    /// alpha not yet valued. An event that does not decode is an error, never skipped: a
    /// runtime that changed an event's shape must stop the watcher, not lose deposits.
    async fn payments(
        &self,
        number: u64,
    ) -> anyhow::Result<(String, Vec<([u8; 32], bool, Deposit)>)> {
        let at = self.0.at_block(number).await?;
        let hash = format!("0x{}", hex::encode(at.block_hash().0));
        let mut out = Vec::new();
        for ev in at.events().fetch().await?.iter() {
            let ev = ev?;
            let (to, from, netuid, same_subnet, rao) = match (ev.pallet_name(), ev.event_name()) {
                ("Balances", "Transfer") => {
                    let (from, to, rao) =
                        ev.decode_fields_unchecked_as::<([u8; 32], [u8; 32], u64)>()?;
                    (to, from, None, false, rao)
                }
                ("SubtensorModule", "StakeTransferred") => {
                    let (from, to, _hotkey, origin, netuid, rao) = ev
                        .decode_fields_unchecked_as::<([u8; 32], [u8; 32], [u8; 32], u16, u16, u64)>()
                        ?;
                    // Across subnets the amount is TAO a real swap paid out; root alpha is TAO.
                    (to, from, Some(netuid), origin == netuid && netuid != 0, rao)
                }
                _ => continue,
            };
            let deposit = Deposit {
                index: ev.index(),
                from: ss58(&from),
                netuid,
                rao,
                valued_rao: rao,
            };
            out.push((to, same_subnet, deposit));
        }
        Ok((hash, out))
    }

    /// Block `number`'s hash and what it paid `treasury`. The caller keeps `number` at or below
    /// [`Chain::finalized`].
    pub async fn deposits(
        &self,
        number: u64,
        treasury: &str,
    ) -> anyhow::Result<(String, Vec<Deposit>)> {
        let treasury = crate::wallets::public_key(treasury)
            .ok_or_else(|| anyhow::anyhow!("not a Bittensor address"))?;
        let (hash, payments) = self.payments(number).await?;
        let mut out = Vec::new();
        for (to, same_subnet, mut deposit) in payments {
            if to != treasury {
                continue;
            }
            if let (true, Some(netuid)) = (same_subnet, deposit.netuid) {
                let before = number.saturating_sub(PRICE_LOOKBACK);
                deposit.valued_rao = alpha_value(
                    deposit.rao,
                    self.alpha_price(number, netuid).await?,
                    self.alpha_price(before, netuid).await?,
                );
            }
            out.push(deposit);
        }
        Ok((hash, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_worth_the_lower_of_two_prices() {
        // Steady or fallen price: what the chain said. Pumped tenfold for the block: a tenth.
        assert_eq!(alpha_value(1_000, 50, 50), 1_000);
        assert_eq!(alpha_value(1_000, 50, 80), 1_000);
        assert_eq!(alpha_value(1_000, 500, 50), 100);
        assert_eq!(alpha_value(1_000, 0, 50), 0);
        assert_eq!(alpha_value(u64::MAX, u64::MAX, u64::MAX), u64::MAX);
    }

    /// Live: recent finalized Finney blocks decode, and hold both kinds of payment.
    /// `ARBOS_CHAIN_RPC=https://archive.sub.latent.to cargo test -p arbos-relay live_ -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_recent_blocks_decode() {
        let url = std::env::var("ARBOS_CHAIN_RPC").expect("ARBOS_CHAIN_RPC");
        let chain = Chain::connect(&url, reqwest::Client::new()).await.unwrap();
        let head = chain.finalized().await.unwrap();
        let (mut tao, mut alpha) = (0, 0);
        for n in (head - 40..=head).rev() {
            for (to, same_subnet, d) in chain.payments(n).await.unwrap().1 {
                match d.netuid {
                    None => tao += 1,
                    Some(netuid) => {
                        alpha += 1;
                        let price = chain.alpha_price(n, netuid).await.unwrap();
                        println!(
                            "{n}#{} sn{netuid} {} rao same={same_subnet} price={price} to {}",
                            d.index,
                            d.rao,
                            ss58(&to)
                        );
                        // And through the public door, as that payee's treasury.
                        let (_, mine) = chain.deposits(n, &ss58(&to)).await.unwrap();
                        assert!(mine
                            .iter()
                            .any(|m| m.index == d.index && m.valued_rao <= d.rao));
                    }
                }
            }
        }
        println!("{tao} TAO transfers, {alpha} stake transfers in 41 blocks");
        assert!(tao > 0, "no TAO transfer in 41 blocks");
    }
}
