//! Reading the Bittensor chain for the deposit watcher (`deposits.rs`): finalized blocks only,
//! over plain JSON-RPC on HTTPS. Nothing here signs or holds a key. The wallet crate has a
//! chain client of its own, but it is the apps' alone (CI checks its dependents), so the relay
//! reads the chain itself.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::value::RawValue;
use subxt::backend::LegacyBackend;
use subxt::config::SubstrateConfig;
use subxt::events::Phase;
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

/// TAO that arrived at a treasury address in one finalized block, read from the block's
/// events, so a transfer made through a proxy, a batch or a multisig is seen like any other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deposit {
    /// The event's position in its block: with the block hash, the deposit's identity.
    pub index: u32,
    /// The account the TAO left.
    pub from: String,
    /// What moved, in rao.
    pub rao: u64,
}

/// The positions of the transfers that count, given each event's (extrinsic, pallet, name) in
/// block order. A `Balances.Transfer` counts when it ran in an extrinsic that emitted no
/// `SubtensorModule` event. Handing stake to another coldkey across subnets pays that coldkey
/// TAO and stakes it away again in the same extrinsic (Subtensor `transition_stake_internal`),
/// so the treasury is left holding alpha, not the TAO the transfer shows. Every stake call
/// emits a `SubtensorModule` event; a plain transfer, alone or in a batch, a proxy or a
/// multisig, emits none. A transfer outside any extrinsic is nobody's payment.
fn transfers_that_count(events: &[(Option<u32>, &str, &str)]) -> Vec<usize> {
    let staking: HashSet<u32> = events
        .iter()
        .filter(|e| e.1 == "SubtensorModule")
        .filter_map(|e| e.0)
        .collect();
    (0..events.len())
        .filter(|&i| {
            let (extrinsic, pallet, name) = events[i];
            (pallet, name) == ("Balances", "Transfer")
                && extrinsic.is_some_and(|x| !staking.contains(&x))
        })
        .collect()
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

    /// Block `number`'s hash and every TAO payment in it, as `(to, deposit)`. An event that
    /// does not decode is an error, never skipped: a runtime that changed an event's shape must
    /// stop the watcher, not lose deposits.
    async fn payments(&self, number: u64) -> anyhow::Result<(String, Vec<([u8; 32], Deposit)>)> {
        let at = self.0.at_block(number).await?;
        let hash = format!("0x{}", hex::encode(at.block_hash().0));
        let events = at.events().fetch().await?;
        let events = events.iter().collect::<Result<Vec<_>, _>>()?;
        let kinds: Vec<_> = events
            .iter()
            .map(|ev| {
                let extrinsic = match ev.phase() {
                    Phase::ApplyExtrinsic(i) => Some(i),
                    _ => None,
                };
                (extrinsic, ev.pallet_name(), ev.event_name())
            })
            .collect();
        let mut out = Vec::new();
        for i in transfers_that_count(&kinds) {
            let ev = &events[i];
            let (from, to, rao) = ev.decode_fields_unchecked_as::<([u8; 32], [u8; 32], u64)>()?;
            // A transfer of nothing is not a deposit.
            if rao > 0 {
                let from = ss58(&from);
                out.push((
                    to,
                    Deposit {
                        index: ev.index(),
                        from,
                        rao,
                    },
                ));
            }
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
        let paid = payments.into_iter().filter(|(to, _)| *to == treasury);
        Ok((hash, paid.map(|(_, deposit)| deposit).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_beside_a_stake_event_in_its_extrinsic_does_not_count() {
        let transfer = |x| (x, "Balances", "Transfer");
        let staked = |x| (x, "SubtensorModule", "StakeTransferred");
        let fee = (Some(1), "TransactionPayment", "TransactionFeePaid");
        assert_eq!(transfers_that_count(&[transfer(Some(1)), fee]), [0]);
        assert!(transfers_that_count(&[transfer(Some(1)), staked(Some(1))]).is_empty());
        assert!(transfers_that_count(&[staked(Some(1)), transfer(Some(1))]).is_empty());
        // The same two in different extrinsics of one block.
        assert_eq!(
            transfers_that_count(&[transfer(Some(1)), staked(Some(2))]),
            [0]
        );
        // Outside any extrinsic.
        assert!(transfers_that_count(&[transfer(None)]).is_empty());
    }

    /// Live: recent finalized Finney blocks decode, and hold TAO transfers.
    /// `ARBOS_CHAIN_RPC=https://archive.sub.latent.to cargo test -p arbos-relay live_ -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_recent_blocks_decode() {
        let url = std::env::var("ARBOS_CHAIN_RPC").expect("ARBOS_CHAIN_RPC");
        let chain = Chain::connect(&url, reqwest::Client::new()).await.unwrap();
        let head = chain.finalized().await.unwrap();
        let mut tao = 0;
        for n in (head - 40..=head).rev() {
            for (to, d) in chain.payments(n).await.unwrap().1 {
                tao += 1;
                // And through the public door, as that payee's treasury.
                let (_, mine) = chain.deposits(n, &ss58(&to)).await.unwrap();
                assert!(mine.contains(&d));
            }
        }
        println!("{tao} TAO transfers in 41 blocks");
        assert!(tao > 0, "no TAO transfer in 41 blocks");
    }
}
