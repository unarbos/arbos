//! The chain: balances, and **offline cheques** — TAO transfers signed with no network at all.
//!
//! A cheque is a `Balances.transfer_keep_alive` extrinsic signed from a [`Snapshot`] the device
//! saved while it was last online: mortal (it expires [`CHEQUE_PERIOD`] blocks after the
//! snapshot's block), at an explicit nonce. It travels as bytes (a chat message, a QR code): the
//! signed extrinsic followed by the block it is anchored to (number as u32 LE, then the 32-byte
//! hash), which a reader needs to check the signature and the expiry with no network. Whoever
//! gets online first submits it, and the nonce makes it single-use. What a cheque pays is always
//! read back from the signed bytes ([`inspect`]), never from the message that carried it, and
//! it pays nothing until it is cashed. It can bounce: the payer's balance ran short, another
//! transaction took that nonce first, or it expired.
//!
//! One account's cheques cash in nonce order: a later cheque waits until every earlier one is
//! cashed or has expired and had its number reused.
//!
//! Chain access is plain JSON-RPC over HTTPS (no websocket), through the same HTTP stack as
//! `arbos-matrix`.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use subxt::backend::LegacyBackend;
use subxt::client::OfflineClientAtBlock;
use subxt::config::substrate::{
    SpecVersionForRange, SubstrateConfig, SubstrateExtrinsicParamsBuilder,
};
use subxt::config::transaction_extensions::CheckMortality;
use subxt::config::DefaultExtrinsicParams;
use subxt::dynamic::Value;
use subxt::transactions::ValidationResult;
use subxt::utils::{AccountId32, Era, MultiSignature, H256};
use subxt::{OfflineClient, OnlineClient};
use subxt_signer::sr25519;
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

/// Signed cheques are ~190 bytes; anything far bigger is not one.
const MAX_CHEQUE: usize = 1024;
/// Polls of 4 s: three minutes for a transfer to finalize.
const POLLS: u32 = 45;
/// Subtensor's block time, for turning block counts into clock time. Estimates only: blocks can
/// run late, never meaningfully early.
const BLOCK_SECS: u64 = 12;
/// How many blocks a cheque can be cashed for, counted from the snapshot block it is anchored
/// to: about 6 h 50 min. It must be a power of two (the era encoding) and cannot usefully exceed
/// Subtensor's `System.BlockHashCount`, 2400 blocks (8 h; read from Finney's metadata): the
/// chain forgets older block hashes, and a cheque anchored to a forgotten block is dead whatever
/// its era says. 2048 is the largest that fits, so "good for a day" is not on offer here.
pub const CHEQUE_PERIOD: u64 = 2048;
/// A snapshot whose block is older than this (by this device's clock) signs nothing: the cheque
/// would be born with under half its life left. So signing "offline" means "was online in the
/// last ~3.4 h". A clock set backwards makes a snapshot look fresh; the cheque is then simply
/// born nearer its expiry, or expired.
const MAX_ANCHOR_AGE: u64 = CHEQUE_PERIOD / 2;
/// The tail of a cheque after the extrinsic: anchor block number (u32 LE) and hash.
const ANCHOR: usize = 4 + 32;
/// The signature binds a cheque's anchor block by hash, and its number only modulo the era. A
/// reader refuses an anchor more than this many blocks ahead of its own estimate of the chain,
/// so nobody relaying a cheque can push its expiry out by whole eras (an anchor moved back
/// instead only reads as expiring sooner). Roomy enough for clock skew: 20 minutes.
const ANCHOR_SLACK: u64 = 100;
/// How far past a pending cheque's submission block a later look searches for it.
const WATCH: u64 = 50;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---- offline ----------------------------------------------------------------------------------

/// Everything offline signing needs, captured while online. Not secret, not per-account. One
/// snapshot signs cheques for [`MAX_ANCHOR_AGE`] blocks and reads them for as long as the
/// runtime does not upgrade; the app takes a new one whenever it is online.
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub net: Net,
    pub genesis: String,
    pub spec_version: u32,
    pub tx_version: u32,
    /// SCALE metadata, 0x-hex (a few hundred KB).
    pub metadata: String,
    /// A finalized block when the snapshot was taken, and its hash: cheques are anchored to it
    /// and expire [`CHEQUE_PERIOD`] blocks later.
    pub block: u64,
    pub block_hash: String,
    /// This device's clock (unix seconds) when it was taken: how `block` maps to time.
    pub at: u64,
}

fn h256(hex: &str) -> crate::Result<H256> {
    let bytes: [u8; 32] = unhex(hex)?.try_into().map_err(|_| err("bad block hash"))?;
    Ok(H256(bytes))
}

fn offline(snap: &Snapshot) -> crate::Result<OfflineClient<SubstrateConfig>> {
    let meta = unhex(&snap.metadata)?;
    let metadata = subxt::metadata::Metadata::decode_from(&meta[..]).map_err(err)?;
    let config = SubstrateConfig::builder()
        .set_genesis_hash(h256(&snap.genesis)?)
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

/// Mortal from `block`, at an explicit nonce, no tip: the only shape a cheque has.
fn params(
    nonce: u64,
    period: u64,
    block: u64,
    hash: H256,
) -> DefaultExtrinsicParams<SubstrateConfig> {
    SubstrateExtrinsicParamsBuilder::new()
        .nonce(nonce)
        .mortal_from_unchecked(period, block, hash)
        .build()
}

/// What a cheque pays, read from its signed bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cheque {
    pub from: String,
    pub to: String,
    pub rao: u128,
    /// Extrinsic hash, 0x-hex: the cheque's identity on chain.
    pub tx: String,
    /// The payer's account nonce it spends.
    pub nonce: u64,
    /// The first block that can no longer include it.
    pub expires_block: u64,
    /// When that block is due (unix seconds), estimated from the reader's snapshot.
    pub expires_at: u64,
}

/// Sign a cheque with no network. The caller hands out nonces (the account's next nonce on
/// chain, then the lowest number no outstanding cheque holds) so two cheques never collide.
pub fn sign(
    snap: &Snapshot,
    seed: &Seed,
    to: &str,
    rao: u128,
    nonce: u64,
) -> crate::Result<(Vec<u8>, Cheque)> {
    sign_at(snap, seed, to, rao, nonce, now())
}

fn sign_at(
    snap: &Snapshot,
    seed: &Seed,
    to: &str,
    rao: u128,
    nonce: u64,
    now: u64,
) -> crate::Result<(Vec<u8>, Cheque)> {
    if now.saturating_sub(snap.at) / BLOCK_SECS > MAX_ANCHOR_AGE {
        return Err(Error::Cheque(
            "this device was last online too long ago to sign a cheque; go online once".into(),
        ));
    }
    let to = parse_address(to)?;
    let block = u32::try_from(snap.block).map_err(|_| err("bad block number"))?;
    let hash = h256(&snap.block_hash)?;
    let client = offline(snap)?;
    let at = client.at_block(0u64).map_err(err)?;
    let call = transfer(to, rao);
    let mut partial = at
        .transactions()
        .create_v4_signable_offline(&call, params(nonce, CHEQUE_PERIOD, snap.block, hash))
        .map_err(err)?;
    let mut bytes = partial.sign(&seed.keypair()).map_err(err)?.into_encoded();
    bytes.extend_from_slice(&block.to_le_bytes());
    bytes.extend_from_slice(&hash.0);
    let cheque = inspect_at(&at, snap, &bytes, now)?;
    Ok((bytes, cheque))
}

/// Read and check a cheque that arrived from someone else: it must be exactly one
/// `Balances.transfer_keep_alive` to an account id, mortal, and signed by its sender — the
/// sr25519 signature is verified here, so nothing unsigned or altered reads as a cheque. That
/// it is funded and unspent only the chain can say: a cheque is not money until cashed.
pub fn inspect(snap: &Snapshot, bytes: &[u8]) -> crate::Result<Cheque> {
    let client = offline(snap)?;
    let at = client.at_block(0u64).map_err(err)?;
    inspect_at(&at, snap, bytes, now())
}

/// A cheque's extrinsic and the block it is anchored to.
fn split(bytes: &[u8]) -> crate::Result<(&[u8], u64, H256)> {
    let bad = || Error::Cheque("not a cheque".into());
    if bytes.len() <= ANCHOR || bytes.len() > MAX_CHEQUE {
        return Err(bad());
    }
    let (ext, anchor) = bytes.split_at(bytes.len() - ANCHOR);
    let block = u32::from_le_bytes(anchor[..4].try_into().map_err(|_| bad())?);
    let hash: [u8; 32] = anchor[4..].try_into().map_err(|_| bad())?;
    Ok((ext, block.into(), H256(hash)))
}

fn inspect_at(
    at: &OfflineClientAtBlock<SubstrateConfig>,
    snap: &Snapshot,
    bytes: &[u8],
    now: u64,
) -> crate::Result<Cheque> {
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
    let (raw, block, hash) = split(bytes)?;
    if block > snap.block + now.saturating_sub(snap.at) / BLOCK_SECS + ANCHOR_SLACK {
        return Err(bad("it is dated in the future"));
    }
    // Offline decoding awaits nothing real; drive it in place.
    let exts = futures::executor::block_on(at.extrinsics().from_bytes(vec![raw.to_vec()]));
    let ext = exts
        .iter()
        .next()
        .ok_or_else(|| bad("empty"))?
        .map_err(|e| bad(&e.to_string()))?;
    if (ext.pallet_name(), ext.call_name()) != ("Balances", "transfer_keep_alive") {
        return Err(bad("not a plain TAO transfer"));
    }
    // MultiAddress::Id is variant 0 then the 32-byte account; MultiSignature::Sr25519 is
    // variant 1 then 64 bytes. Nothing else is a cheque.
    let from: [u8; 32] = match ext.address_bytes() {
        Some([0, account @ ..]) => account.try_into().map_err(|_| bad("unsigned"))?,
        _ => return Err(bad("unsigned")),
    };
    let sig: [u8; 64] = match ext.signature_bytes() {
        Some([1, sig @ ..]) => sig.try_into().map_err(|_| bad("unsigned"))?,
        _ => return Err(bad("unsigned")),
    };
    let Fields {
        dest: Dest::Id(to),
        value,
    } = ext
        .decode_call_data_fields_unchecked_as()
        .map_err(|e| bad(&e.to_string()))?;
    let signed = ext
        .transaction_extensions()
        .ok_or_else(|| bad("unsigned"))?;
    let nonce = signed.nonce().ok_or_else(|| bad("no nonce"))?;
    // Longer eras quantize their start, so the anchor would not be the block they hash.
    let period = match signed.find::<CheckMortality<SubstrateConfig>>() {
        Some(Ok(Era::Mortal { period, .. })) if period <= 4096 => period,
        _ => return Err(bad("it never expires")),
    };
    // Rebuild the one extrinsic these facts describe. The signature must sign its payload, and
    // it must be the received bytes exactly: no tip, no other era, nothing unaccounted for.
    let call = transfer(to.0, value);
    let mut partial = at
        .transactions()
        .create_v4_signable_offline(&call, params(nonce, period, block, hash))
        .map_err(err)?;
    let payload = partial.signer_payload().map_err(err)?;
    if !sr25519::verify(&sr25519::Signature(sig), payload, &sr25519::PublicKey(from)) {
        return Err(bad("the signature is not the payer's"));
    }
    let rebuilt = partial
        .sign_with_account_and_signature(&AccountId32(from), &MultiSignature::Sr25519(sig))
        .map_err(err)?;
    if rebuilt.encoded() != raw {
        return Err(bad("not a plain TAO transfer"));
    }
    let expires_block = block + period;
    let secs = |blocks: u64| blocks.saturating_mul(BLOCK_SECS);
    Ok(Cheque {
        from: ss58(&from),
        to: ss58(&to.0),
        rao: value,
        tx: format!("0x{}", hex::encode(ext.hash().0)),
        nonce,
        expires_block,
        expires_at: if expires_block >= snap.block {
            snap.at.saturating_add(secs(expires_block - snap.block))
        } else {
            snap.at.saturating_sub(secs(snap.block - expires_block))
        },
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
    // Finalized, so the block a cheque is anchored to cannot be reorganised away.
    let head = call("chain_getFinalizedHead", subxt::rpcs::client::rpc_params![])
        .await
        .map_err(err)?;
    let header = call("chain_getHeader", subxt::rpcs::client::rpc_params![&head])
        .await
        .map_err(err)?;
    let block = header["number"]
        .as_str()
        .and_then(|n| u64::from_str_radix(n.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| err("bad block header"))?;
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
            block,
            block_hash: text(head)?,
            at: now(),
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

/// What became of a cheque handed to the chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cashed {
    /// It succeeded in a finalized block: the money moved.
    Paid(Receipt),
    /// The chain took it but it was not seen finalized (yet): not a bounce. Ask again later,
    /// passing `since` back so the blocks from then on are searched for it.
    Pending { since: u64 },
}

/// Look for the extrinsic in one block: `Some` once it is there (paid, or failed on chain).
async fn find(api: &Api, number: u64, hash: H256) -> crate::Result<Option<crate::Result<Receipt>>> {
    let at = api.at_block(number).await.map_err(err)?;
    for ext in at.extrinsics().fetch().await.map_err(err)?.iter() {
        let ext = ext.map_err(err)?;
        if ext.hash() != hash {
            continue;
        }
        for ev in ext.events().await.map_err(err)?.iter() {
            let ev = ev.map_err(err)?;
            if (ev.pallet_name(), ev.event_name()) == ("System", "ExtrinsicFailed") {
                return Ok(Some(Err(Error::Cheque("bounced on chain".into()))));
            }
        }
        return Ok(Some(Ok(Receipt {
            tx: format!("0x{}", hex::encode(hash.0)),
            block: format!("0x{}", hex::encode(at.block_hash().0)),
        })));
    }
    Ok(None)
}

/// Submit a cheque (payer or payee may) and wait until it succeeded in a finalized block.
/// `since` is what an earlier [`Cashed::Pending`] returned for this cheque. `Err` is a real
/// refusal; anything uncertain after the chain accepted it is `Pending`, never an error.
pub async fn cash(net: Net, bytes: &[u8], since: Option<u64>) -> crate::Result<Cashed> {
    let (raw, _, _) = split(bytes)?;
    let api = api(net).await?;
    let at = api.at_current_block().await.map_err(err)?;
    let head = at.block_number();
    let tx = at.transactions().from_bytes(raw.to_vec());
    let hash = tx.hash();
    // Where an earlier attempt left it: it may have landed while nobody was watching.
    if let Some(since) = since {
        for n in since..=head.min(since.saturating_add(WATCH)) {
            if let Some(found) = find(&api, n, hash).await? {
                return found.map(Cashed::Paid);
            }
        }
    }
    // The chain's own checks first, so a bounce says why instead of never landing.
    match tx.validate().await.map_err(err)? {
        ValidationResult::Invalid(why) => {
            return Err(Error::Cheque(bounce(&format!("{why:?}"))));
        }
        // Valid but behind another nonce: the pool would only park it.
        ValidationResult::Valid(v) if !v.requires.is_empty() => {
            return Err(Error::Cheque(
                "an earlier cheque from this payer has to be cashed first".into(),
            ));
        }
        _ => {}
    }
    // Plain submit: subxt's own `submit` watches the transaction, a subscription HTTP can't carry.
    let hex = format!("0x{}", hex::encode(raw));
    if let Err(e) = rpc(net)?
        .request::<serde_json::Value>(
            "author_submitExtrinsic",
            subxt::rpcs::client::rpc_params![hex],
        )
        .await
    {
        // Asked again about a pending cheque: the pool may hold it, or a block not yet
        // finalized may; either way it is still pending from where it was.
        if let Some(since) = since {
            return Ok(Cashed::Pending { since });
        }
        return Err(err(e));
    }
    // From here the chain has it: a failed poll or a timeout leaves it pending, not bounced.
    let pending = Cashed::Pending { since: head };
    let mut next = head;
    for _ in 0..POLLS {
        let Ok(now) = api.at_current_block().await else {
            return Ok(pending);
        };
        while next <= now.block_number() {
            match find(&api, next, hash).await {
                Ok(Some(found)) => return found.map(Cashed::Paid),
                Ok(None) => next += 1,
                Err(_) => return Ok(pending),
            }
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
    }
    Ok(pending)
}

/// The chain's refusal in words a person can act on. A cheque past its era fails the
/// signature check (the chain hashes a later block than the one signed), so `BadProof` is
/// usually "expired"; once the anchor block's hash is forgotten it is `AncientBirthBlock`.
fn bounce(why: &str) -> String {
    if why.contains("Stale") {
        "already cashed, or the payer used that cheque number for something else".into()
    } else if why.contains("Payment") || why.contains("Funds") {
        "the payer's balance is too low".into()
    } else if why.contains("BadProof") || why.contains("Ancient") {
        "it expired (or the chain upgraded since it was signed); ask the payer for a new one".into()
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

    /// A snapshot over Finney's real metadata (`tests/finney-metadata.scale`, fetched with
    /// `state_getMetadata`), so signing and reading are tested with no network. The block,
    /// hashes and versions are made up: nothing offline checks them against a chain.
    fn snapshot_fixture() -> Snapshot {
        Snapshot {
            net: Net::Main,
            genesis: format!("0x{}", "11".repeat(32)),
            spec_version: 1,
            tx_version: 1,
            metadata: hex::encode(include_bytes!("../tests/finney-metadata.scale")),
            block: 9_481_389,
            block_hash: format!("0x{}", "22".repeat(32)),
            at: 1_800_000_000,
        }
    }

    /// A cheque signed with any params, to make the shapes `sign` never does.
    fn cheque_with(
        snap: &Snapshot,
        payer: &Seed,
        to: &str,
        params: DefaultExtrinsicParams<SubstrateConfig>,
    ) -> Vec<u8> {
        let at = offline(snap).unwrap().at_block(0u64).unwrap();
        let call = transfer(parse_address(to).unwrap(), 1_500_000_000);
        let mut partial = at
            .transactions()
            .create_v4_signable_offline(&call, params)
            .unwrap();
        let mut bytes = partial.sign(&payer.keypair()).unwrap().into_encoded();
        bytes.extend_from_slice(&(snap.block as u32).to_le_bytes());
        bytes.extend_from_slice(&h256(&snap.block_hash).unwrap().0);
        bytes
    }

    #[test]
    fn a_cheque_reads_back_with_its_signer_and_expiry() {
        let snap = snapshot_fixture();
        let (payer, payee) = (Seed::generate(), Seed::generate().address());
        let (bytes, signed) =
            sign_at(&snap, &payer, &payee, 1_500_000_000, 7, snap.at + 60).unwrap();
        assert_eq!(
            signed,
            Cheque {
                from: payer.address(),
                to: payee,
                rao: 1_500_000_000,
                tx: signed.tx.clone(),
                nonce: 7,
                expires_block: snap.block + CHEQUE_PERIOD,
                expires_at: snap.at + CHEQUE_PERIOD * BLOCK_SECS,
            }
        );
        // A reader whose own snapshot is 100 blocks later puts the expiry at the same moment.
        let later = Snapshot {
            block: snap.block + 100,
            at: snap.at + 100 * BLOCK_SECS,
            ..snap
        };
        assert_eq!(inspect(&later, &bytes).unwrap(), signed);
    }

    #[test]
    fn nothing_altered_or_unsigned_reads_as_a_cheque() {
        let snap = snapshot_fixture();
        let at = offline(&snap).unwrap().at_block(0u64).unwrap();
        let (payer, payee) = (Seed::generate(), Seed::generate().address());
        let (bytes, _) = sign_at(&snap, &payer, &payee, 1_500_000_000, 0, snap.at).unwrap();
        // Every byte is covered: signature, amount, payee, nonce, era, anchor block and hash.
        let (body, number) = (bytes.len() - ANCHOR, bytes.len() - ANCHOR..bytes.len() - 32);
        for i in (0..bytes.len()).filter(|i| !number.contains(i)) {
            let mut forged = bytes.clone();
            forged[i] ^= 1;
            assert!(
                inspect_at(&at, &snap, &forged, snap.at).is_err(),
                "byte {i}"
            );
        }
        // The anchor's number is signed only modulo the era: off by anything else is refused,
        // a whole era later is refused as not yet possible, and a whole era earlier can only
        // make the cheque read as expiring sooner, never later.
        let moved = |by: i64| {
            let mut forged = bytes.clone();
            let n = (snap.block as i64 + by) as u32;
            forged[body..body + 4].copy_from_slice(&n.to_le_bytes());
            inspect_at(&at, &snap, &forged, snap.at).map(|c| c.expires_block)
        };
        assert!(moved(1).is_err());
        assert!(moved(CHEQUE_PERIOD as i64).is_err());
        assert_eq!(moved(-(CHEQUE_PERIOD as i64)), Ok(snap.block));
        assert!(inspect(&snap, &[]).is_err());
        assert!(inspect(&snap, &bytes[..bytes.len() - 1]).is_err());
        assert!(inspect(&snap, &vec![0; MAX_CHEQUE + 1]).is_err());
        // Signed by someone else over the same words.
        let hash = h256(&snap.block_hash).unwrap();
        let mortal = params(0, CHEQUE_PERIOD, snap.block, hash);
        let mut stolen = cheque_with(&snap, &Seed::generate(), &payee, mortal);
        stolen[4..36].copy_from_slice(&parse_address(&payer.address()).unwrap());
        assert!(inspect(&snap, &stolen).is_err());
        // A cheque that never expires is not one.
        let immortal = SubstrateExtrinsicParamsBuilder::new()
            .nonce(0)
            .immortal()
            .build();
        let e = inspect(&snap, &cheque_with(&snap, &payer, &payee, immortal)).unwrap_err();
        assert!(e.to_string().contains("never expires"), "{e}");
    }

    #[test]
    fn an_old_snapshot_signs_nothing() {
        let snap = snapshot_fixture();
        let (payer, payee) = (Seed::generate(), Seed::generate().address());
        let at_age =
            |blocks: u64| sign_at(&snap, &payer, &payee, 1, 0, snap.at + blocks * BLOCK_SECS);
        assert!(at_age(MAX_ANCHOR_AGE).is_ok());
        assert!(matches!(at_age(MAX_ANCHOR_AGE + 1), Err(Error::Cheque(_))));
    }

    /// Live: snapshot Finney, sign a cheque offline, read it back. Needs the network, so it only
    /// runs with `ARBOS_WALLET_LIVE=1`. Nothing is submitted: the chain refuses it at validation.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cheque_signed_from_a_live_snapshot_reads_back() {
        if std::env::var("ARBOS_WALLET_LIVE").as_deref() != Ok("1") {
            return;
        }
        let payer = Seed::generate();
        let payee = Seed::generate().address();
        let (snap, nonce) = snapshot(Net::Main, &payer.address()).await.unwrap();
        assert_eq!(nonce, 0);
        let snap: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        let (bytes, signed) = sign(&snap, &payer, &payee, 1_500_000_000, 0).unwrap();
        assert_eq!(inspect(&snap, &bytes).unwrap(), signed);
        // An empty account's cheque bounces before submission, with a reason.
        let e = cash(Net::Main, &bytes, None).await.unwrap_err();
        assert!(e.to_string().contains("too low"), "{e}");
    }

    /// The whole trip on a local subtensor (`docker run -p 9944:9944
    /// ghcr.io/opentensor/subtensor-localnet True`, then `ARBOS_WALLET_LOCALNET=1`): Alice funds a
    /// fresh wallet, it signs cheques from a snapshot, the payee cashes them in order, the money
    /// moves, the same cheque cannot be cashed twice, and an expired one is refused.
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
        let mut funding = tx.encoded().to_vec();
        funding.extend_from_slice(&[0; ANCHOR]);
        assert!(matches!(
            cash(Net::Local, &funding, None).await.unwrap(),
            Cashed::Paid(_)
        ));

        // Offline from here: a snapshot is all the payer needs to sign.
        let (snap, nonce) = snapshot(Net::Local, &payer.address()).await.unwrap();
        let (bytes, cheque) = sign(&snap, &payer, &payee, 1_500_000_000, nonce).unwrap();
        let (second, _) = sign(&snap, &payer, &payee, 1, nonce + 1).unwrap();
        assert_eq!(inspect(&snap, &bytes).unwrap(), cheque);

        let early = cash(Net::Local, &second, None).await.unwrap_err();
        assert!(early.to_string().contains("cashed first"), "{early}");
        let Cashed::Paid(receipt) = cash(Net::Local, &bytes, None).await.unwrap() else {
            panic!("still pending after three minutes");
        };
        assert_eq!(receipt.tx, cheque.tx);
        assert_eq!(balance(Net::Local, &payee).await.unwrap(), 1_500_000_000);
        let again = cash(Net::Local, &bytes, None).await.unwrap_err();
        assert!(again.to_string().contains("already cashed"), "{again}");
        // Asked again as a pending cheque, it is found where it landed.
        let since = snap.block;
        assert_eq!(
            cash(Net::Local, &bytes, Some(since)).await.unwrap(),
            Cashed::Paid(receipt)
        );

        // A 4-block cheque, cashed after its era: refused, and the payee gets nothing more.
        let hash = h256(&snap.block_hash).unwrap();
        let short = cheque_with(
            &snap,
            &payer,
            &payee,
            params(nonce + 1, 4, snap.block, hash),
        );
        assert_eq!(
            inspect(&snap, &short).unwrap().expires_block,
            snap.block + 4
        );
        let expired = cash(Net::Local, &short, None).await.unwrap_err();
        assert!(expired.to_string().contains("expired"), "{expired}");
        assert_eq!(balance(Net::Local, &payee).await.unwrap(), 1_500_000_000);
    }

    #[test]
    fn bounces_read_like_people_talk() {
        assert!(bounce("Invalid(Stale)").contains("already cashed"));
        assert!(bounce("Invalid(Payment)").contains("too low"));
        assert!(bounce("Invalid(BadProof)").contains("expired"));
    }
}
