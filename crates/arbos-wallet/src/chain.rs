//! The chain: balances, and **offline cheques** — TAO transfers signed with no network at all.
//!
//! A cheque is a `Balances.transfer_keep_alive` extrinsic signed from a [`Snapshot`] the device
//! saved while it was last online: immortal, at an explicit nonce. It travels as bytes (a chat
//! message, a QR code); whoever gets online first submits it, and the nonce makes it single-use.
//! What a cheque pays is always read back from the signed bytes ([`inspect`]), never from the
//! message that carried it. It can bounce: the payer's balance ran short, or another
//! transaction took that nonce first.
//!
//! Chain access is plain JSON-RPC over HTTPS (no websocket), through the same HTTP stack as
//! `arbos-matrix`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use subxt::backend::LegacyBackend;
use subxt::client::OfflineClientAtBlock;
use subxt::config::substrate::{SpecVersionForRange, SubstrateConfig};
use subxt::dynamic::Value;
use subxt::utils::{AccountId32, H256};
use subxt::{OfflineClient, OnlineClient};
use tokio::sync::Mutex;

use crate::{parse_address, ss58, Error, Seed};

// `crate::Result` is spelled out, never imported: the scale-decode derives below expand a bare
// `Result` and must get std's.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Net {
    /// Finney, Bittensor's main network.
    Main,
    Test,
    /// `ghcr.io/opentensor/subtensor-localnet` on this machine, for the cashing test.
    #[cfg(test)]
    Local,
}

impl Net {
    fn url(self) -> &'static str {
        match self {
            Net::Main => "https://entrypoint-finney.opentensor.ai:443",
            Net::Test => "https://test.finney.opentensor.ai:443",
            #[cfg(test)]
            Net::Local => "http://127.0.0.1:9944",
        }
    }
}

fn err(e: impl std::fmt::Display) -> Error {
    Error::Chain(e.to_string())
}

/// Signed cheques are ~150 bytes; anything far bigger is not one.
const MAX_CHEQUE: usize = 1024;
/// Polls of 4 s: three minutes for a transfer to finalize.
const POLLS: u32 = 45;

// ---- offline ----------------------------------------------------------------------------------

/// Everything offline signing needs, captured while online. Not secret, not per-account: one
/// snapshot signs every cheque until the runtime upgrades (then cheques stop validating and the
/// app takes a new one).
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub net: Net,
    pub genesis: String,
    pub spec_version: u32,
    pub tx_version: u32,
    /// SCALE metadata, 0x-hex (a few hundred KB).
    pub metadata: String,
}

fn offline(snap: &Snapshot) -> crate::Result<OfflineClient<SubstrateConfig>> {
    let meta = unhex(&snap.metadata)?;
    let metadata = subxt::metadata::Metadata::decode_from(&meta[..]).map_err(err)?;
    let genesis: [u8; 32] = unhex(&snap.genesis)?
        .try_into()
        .map_err(|_| err("bad genesis hash"))?;
    let config = SubstrateConfig::builder()
        .set_genesis_hash(H256(genesis))
        .set_metadata_for_spec_versions([(snap.spec_version, metadata.into())])
        .set_spec_version_for_block_ranges([SpecVersionForRange {
            block_range: 0..u64::MAX,
            spec_version: snap.spec_version,
            transaction_version: snap.tx_version,
        }])
        .build();
    Ok(OfflineClient::new_with_config(config))
}

fn transfer(to: [u8; 32], rao: u128) -> impl subxt::transactions::Payload {
    subxt::dynamic::tx(
        "Balances",
        "transfer_keep_alive",
        vec![
            Value::unnamed_variant("Id", [Value::from_bytes(to)]),
            Value::u128(rao),
        ],
    )
}

/// What a cheque pays, read from its signed bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cheque {
    pub from: String,
    pub to: String,
    pub rao: u128,
    /// Extrinsic hash, 0x-hex: the cheque's identity on chain.
    pub tx: String,
}

/// Sign a cheque with no network. The caller hands out nonces (the snapshot's account nonce,
/// then +1 per cheque) so two cheques never collide.
pub fn sign(
    snap: &Snapshot,
    seed: &Seed,
    to: &str,
    rao: u128,
    nonce: u64,
) -> crate::Result<(Vec<u8>, Cheque)> {
    let to = parse_address(to)?;
    let client = offline(snap)?;
    let at = client.at_block(0u64).map_err(err)?;
    let call = transfer(to, rao);
    let params = subxt::config::substrate::SubstrateExtrinsicParamsBuilder::new()
        .nonce(nonce)
        .immortal()
        .build();
    let mut partial = at
        .transactions()
        .create_signable_offline(&call, params)
        .map_err(err)?;
    let bytes = partial.sign(&seed.keypair()).map_err(err)?.into_encoded();
    let cheque = inspect_at(&at, &bytes)?;
    Ok((bytes, cheque))
}

/// Read and check a cheque that arrived from someone else: it must be exactly one signed
/// `Balances.transfer_keep_alive` to an account id.
pub fn inspect(snap: &Snapshot, bytes: &[u8]) -> crate::Result<Cheque> {
    let client = offline(snap)?;
    let at = client.at_block(0u64).map_err(err)?;
    inspect_at(&at, bytes)
}

fn inspect_at(at: &OfflineClientAtBlock<SubstrateConfig>, bytes: &[u8]) -> crate::Result<Cheque> {
    #[derive(subxt::ext::scale_decode::DecodeAsType)]
    #[decode_as_type(crate_path = "subxt::ext::scale_decode")]
    enum Dest {
        Id(AccountId32),
    }
    #[derive(subxt::ext::scale_decode::DecodeAsType)]
    #[decode_as_type(crate_path = "subxt::ext::scale_decode")]
    struct Fields {
        dest: Dest,
        value: u128,
    }
    let bad = |why: &str| Error::Cheque(why.to_owned());
    if bytes.is_empty() || bytes.len() > MAX_CHEQUE {
        return Err(bad("not a cheque"));
    }
    // Offline decoding awaits nothing real; drive it in place.
    let exts = futures::executor::block_on(at.extrinsics().from_bytes(vec![bytes.to_vec()]));
    let ext = exts
        .iter()
        .next()
        .ok_or_else(|| bad("empty"))?
        .map_err(|e| bad(&e.to_string()))?;
    if (ext.pallet_name(), ext.call_name()) != ("Balances", "transfer_keep_alive") {
        return Err(bad("not a plain TAO transfer"));
    }
    // MultiAddress::Id: variant byte 0, then the 32-byte account.
    let from: [u8; 32] = ext
        .address_bytes()
        .and_then(|b| b.get(1..33))
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| bad("unsigned"))?;
    let Fields {
        dest: Dest::Id(to),
        value,
    } = ext
        .decode_call_data_fields_unchecked_as()
        .map_err(|e| bad(&e.to_string()))?;
    Ok(Cheque {
        from: ss58(&from),
        to: ss58(&to.0),
        rao: value,
        tx: format!("0x{}", hex::encode(ext.hash().0)),
    })
}

// ---- online -----------------------------------------------------------------------------------

struct HttpRpc {
    url: &'static str,
    http: reqwest13::Client,
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
                .post(self.url)
                .header("Content-Type", "application/json")
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
        sub: &'a str,
        _params: Option<Box<RawValue>>,
        _unsub: &'a str,
    ) -> subxt::rpcs::client::RawRpcFuture<'a, subxt::rpcs::client::RawRpcSubscription> {
        Box::pin(async move {
            Err(subxt::rpcs::Error::Client(
                format!("{sub}: HTTP has no subscriptions").into(),
            ))
        })
    }
}

fn http() -> crate::Result<reqwest13::Client> {
    reqwest13::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Arbos")
        .build()
        .map_err(err)
}

fn rpc(net: Net) -> crate::Result<subxt::rpcs::RpcClient> {
    Ok(subxt::rpcs::RpcClient::new(HttpRpc {
        url: net.url(),
        http: http()?,
    }))
}

type Api = OnlineClient<SubstrateConfig>;

/// One client per network, reused (it caches chain metadata).
async fn api(net: Net) -> crate::Result<Api> {
    static APIS: OnceLock<Mutex<Vec<(Net, Api)>>> = OnceLock::new();
    let mut apis = APIS.get_or_init(Default::default).lock().await;
    if let Some((_, a)) = apis.iter().find(|(n, _)| *n == net) {
        return Ok(a.clone());
    }
    let backend = LegacyBackend::builder().build(rpc(net)?);
    let a = Api::from_backend(Arc::new(backend)).await.map_err(err)?;
    apis.push((net, a.clone()));
    Ok(a)
}

/// Capture what offline signing needs, plus `address`'s next nonce.
pub async fn snapshot(net: Net, address: &str) -> crate::Result<(Snapshot, u64)> {
    let who = ss58(&parse_address(address)?);
    let rpc = rpc(net)?;
    let call =
        |m: &'static str, p: subxt::rpcs::client::RpcParams| rpc.request::<serde_json::Value>(m, p);
    let metadata = call("state_getMetadata", subxt::rpcs::client::rpc_params![])
        .await
        .map_err(err)?;
    let ver = call(
        "state_getRuntimeVersion",
        subxt::rpcs::client::rpc_params![],
    )
    .await
    .map_err(err)?;
    let genesis = call("chain_getBlockHash", subxt::rpcs::client::rpc_params![0])
        .await
        .map_err(err)?;
    let nonce = call(
        "system_accountNextIndex",
        subxt::rpcs::client::rpc_params![who],
    )
    .await
    .map_err(err)?;
    let num = |v: &serde_json::Value| v.as_u64().ok_or_else(|| err("bad runtime version"));
    let text = |v: serde_json::Value| {
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| err("bad reply"))
    };
    Ok((
        Snapshot {
            net,
            genesis: text(genesis)?,
            spec_version: num(&ver["specVersion"])? as u32,
            tx_version: num(&ver["transactionVersion"])? as u32,
            metadata: text(metadata)?,
        },
        num(&nonce)?,
    ))
}

/// Free balance in rao.
pub async fn balance(net: Net, address: &str) -> crate::Result<u128> {
    #[derive(subxt::ext::scale_decode::DecodeAsType)]
    #[decode_as_type(crate_path = "subxt::ext::scale_decode")]
    struct Info {
        data: Data,
    }
    #[derive(subxt::ext::scale_decode::DecodeAsType)]
    #[decode_as_type(crate_path = "subxt::ext::scale_decode")]
    struct Data {
        free: u128,
    }
    let acc = parse_address(address)?;
    let at = api(net).await?.at_current_block().await.map_err(err)?;
    let addr = subxt::dynamic::storage::<([u8; 32],), Info>("System", "Account");
    let v = at.storage().fetch(addr, (acc,)).await.map_err(err)?;
    Ok(v.decode().map_err(err)?.data.free)
}

/// Where a cheque landed; enough for anyone to check it on chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub tx: String,
    pub block: String,
}

/// Submit a cheque (payer or payee may) and wait until it succeeded in a finalized block.
pub async fn cash(net: Net, bytes: &[u8]) -> crate::Result<Receipt> {
    let api = api(net).await?;
    let at = api.at_current_block().await.map_err(err)?;
    let start = at.block_number();
    let tx = at.transactions().from_bytes(bytes.to_vec());
    // The chain's own checks first, so a bounce says why instead of never landing.
    if let subxt::transactions::ValidationResult::Invalid(why) = tx.validate().await.map_err(err)? {
        return Err(Error::Cheque(bounce(&format!("{why:?}"))));
    }
    // Plain submit: subxt's own `submit` watches the transaction, a subscription HTTP can't carry.
    let hex = format!("0x{}", hex::encode(bytes));
    rpc(net)?
        .request::<serde_json::Value>(
            "author_submitExtrinsic",
            subxt::rpcs::client::rpc_params![hex],
        )
        .await
        .map_err(err)?;
    let hash = tx.hash();
    let mut next = start;
    for _ in 0..POLLS {
        let head = api.at_current_block().await.map_err(err)?.block_number();
        while next <= head {
            let at = api.at_block(next).await.map_err(err)?;
            for ext in at.extrinsics().fetch().await.map_err(err)?.iter() {
                let ext = ext.map_err(err)?;
                if ext.hash() != hash {
                    continue;
                }
                for ev in ext.events().await.map_err(err)?.iter() {
                    let ev = ev.map_err(err)?;
                    if (ev.pallet_name(), ev.event_name()) == ("System", "ExtrinsicFailed") {
                        return Err(Error::Cheque("bounced on chain".into()));
                    }
                }
                return Ok(Receipt {
                    tx: format!("0x{}", hex::encode(hash.0)),
                    block: format!("0x{}", hex::encode(at.block_hash().0)),
                });
            }
            next += 1;
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
    }
    Err(err("not included after three minutes"))
}

/// The chain's refusal in words a person can act on.
fn bounce(why: &str) -> String {
    if why.contains("Stale") {
        "already cashed (or the payer spent that nonce)".into()
    } else if why.contains("Payment") || why.contains("Funds") {
        "the payer's balance is too low".into()
    } else if why.contains("BadProof") {
        "signed for another runtime; ask the payer for a new one".into()
    } else {
        format!("refused: {why}")
    }
}

fn unhex(s: &str) -> crate::Result<Vec<u8>> {
    hex::decode(s.trim_start_matches("0x")).map_err(|_| err("bad hex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live: snapshot Finney, sign a cheque offline, read it back. Needs the network, so it only
    /// runs with `ARBOS_WALLET_LIVE=1`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cheque_signed_offline_reads_back_exactly() {
        if std::env::var("ARBOS_WALLET_LIVE").as_deref() != Ok("1") {
            return;
        }
        let payer = Seed::generate();
        let payee = Seed::generate().address();
        let (snap, nonce) = snapshot(Net::Main, &payer.address()).await.unwrap();
        assert_eq!(nonce, 0);
        let snap: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        let (bytes, signed) = sign(&snap, &payer, &payee, 1_500_000_000, 7).unwrap();
        let read = inspect(&snap, &bytes).unwrap();
        assert_eq!(read, signed);
        assert_eq!(
            (read.from, read.to, read.rao),
            (payer.address(), payee, 1_500_000_000)
        );
        // Tampering with the bytes changes what they say or breaks them; never silently.
        let mut forged = bytes.clone();
        let n = forged.len();
        forged[n - 1] ^= 1;
        assert_ne!(inspect(&snap, &forged).ok(), Some(signed));
        assert!(inspect(&snap, &[]).is_err());
        assert!(inspect(&snap, &vec![0; MAX_CHEQUE + 1]).is_err());
        // An empty account's cheque bounces before submission, with a reason.
        let e = cash(Net::Main, &bytes).await.unwrap_err();
        assert!(matches!(e, Error::Cheque(_)), "{e}");
    }

    /// The whole trip on a local subtensor (`docker run -p 9944:9944
    /// ghcr.io/opentensor/subtensor-localnet True`, then `ARBOS_WALLET_LOCALNET=1`): Alice funds a
    /// fresh wallet, it signs a cheque from a snapshot, the payee cashes it, the money moves, and
    /// the same cheque cannot be cashed twice.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cheque_cashes_once_and_moves_the_money() {
        if std::env::var("ARBOS_WALLET_LOCALNET").as_deref() != Ok("1") {
            return;
        }
        let (payer, payee) = (Seed::generate(), Seed::generate().address());
        let api = api(Net::Local).await.unwrap();
        let fund = transfer(
            parse_address(&payer.address()).unwrap(),
            10 * crate::RAO_PER_TAO,
        );
        let alice = subxt_signer::sr25519::dev::alice();
        let at = api.at_current_block().await.unwrap();
        let tx = at
            .transactions()
            .create_signed(&fund, &alice, Default::default())
            .await
            .unwrap();
        cash(Net::Local, tx.encoded()).await.unwrap();

        // Offline from here: a snapshot is all the payer needs to sign.
        let (snap, nonce) = snapshot(Net::Local, &payer.address()).await.unwrap();
        let (bytes, cheque) = sign(&snap, &payer, &payee, 1_500_000_000, nonce).unwrap();
        assert_eq!(inspect(&snap, &bytes).unwrap(), cheque);

        let receipt = cash(Net::Local, &bytes).await.unwrap();
        assert_eq!(receipt.tx, cheque.tx);
        assert_eq!(balance(Net::Local, &payee).await.unwrap(), 1_500_000_000);
        let again = cash(Net::Local, &bytes).await.unwrap_err();
        assert!(again.to_string().contains("already cashed"), "{again}");
    }

    #[test]
    fn bounces_read_like_people_talk() {
        assert!(bounce("Invalid(Stale)").contains("already cashed"));
        assert!(bounce("Invalid(Payment)").contains("too low"));
    }
}
