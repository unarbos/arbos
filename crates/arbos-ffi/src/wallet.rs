//! The wallet for the Swift client: a thin skin over `arbos-wallet`. The phrase crosses the FFI
//! as a string only to and from the keychain; nothing here stores it.

use arbos_wallet::chain::{self, Net, Snapshot};
use arbos_wallet::Seed;

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum WalletError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Chain(String),
}

impl From<arbos_wallet::Error> for WalletError {
    fn from(e: arbos_wallet::Error) -> Self {
        match e {
            arbos_wallet::Error::Chain(_) => Self::Chain(e.to_string()),
            // A bounce reason is already a sentence for the person ("the payer's balance is
            // too low"); no "cheque:" prefix in front of it.
            arbos_wallet::Error::Cheque(why) => Self::Invalid(why),
            _ => Self::Invalid(e.to_string()),
        }
    }
}

fn seed(phrase: &str) -> Result<Seed, WalletError> {
    Ok(Seed::parse(phrase)?)
}

/// A new 12-word phrase.
#[uniffi::export]
pub fn wallet_new_phrase() -> String {
    Seed::generate().phrase()
}

/// The phrase normalised (spacing, case), or why it is not one.
#[uniffi::export]
pub fn wallet_check_phrase(phrase: String) -> Result<String, WalletError> {
    Ok(seed(&phrase)?.phrase())
}

/// The TAO address the phrase controls.
#[uniffi::export]
pub fn wallet_address(phrase: String) -> Result<String, WalletError> {
    Ok(seed(&phrase)?.address())
}

/// The chat key backup passphrase (`MatrixCrypto.recovery`).
#[uniffi::export]
pub fn wallet_recovery_passphrase(phrase: String) -> Result<String, WalletError> {
    Ok(seed(&phrase)?.recovery_passphrase())
}

/// Proof for `PUT /me/wallet` that `user_id` holds this address.
#[uniffi::export]
pub fn wallet_link_proof(phrase: String, user_id: String) -> Result<String, WalletError> {
    Ok(seed(&phrase)?.link_proof(&user_id))
}

/// Checks an address someone typed or scanned.
#[uniffi::export]
pub fn wallet_check_address(address: String) -> Result<String, WalletError> {
    Ok(arbos_wallet::ss58(&arbos_wallet::parse_address(&address)?))
}

/// "1.5" → rao, strictly.
#[uniffi::export]
pub fn wallet_parse_tao(amount: String) -> Result<String, WalletError> {
    Ok(arbos_wallet::parse_tao(&amount)?.to_string())
}

/// rao (decimal string: u128 does not cross UniFFI) → "1.5 τ".
#[uniffi::export]
pub fn wallet_format_tao(rao: String) -> String {
    rao.parse()
        .map(arbos_wallet::format_tao)
        .unwrap_or_else(|_| "— τ".into())
}

/// Free balance on Finney, in rao.
#[uniffi::export(async_runtime = "tokio")]
pub async fn wallet_balance(address: String) -> Result<String, WalletError> {
    Ok(chain::balance(Net::Main, &address).await?.to_string())
}

/// Taken while online, kept by the app. It signs cheques for a few hours
/// (`arbos_wallet::chain::CHEQUE_PERIOD`), then a newer one is needed; it reads them for longer.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletSnapshot {
    /// Opaque JSON for `wallet_sign_cheque` / `wallet_read_cheque`.
    pub json: String,
    /// The account's next nonce on chain when it was taken.
    pub nonce: u64,
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn wallet_snapshot(address: String) -> Result<WalletSnapshot, WalletError> {
    let (snap, nonce) = chain::snapshot(Net::Main, &address).await?;
    let json = serde_json::to_string(&snap).map_err(|e| WalletError::Chain(e.to_string()))?;
    Ok(WalletSnapshot { json, nonce })
}

/// What a cheque pays, read from its signed bytes.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletCheque {
    /// The signed extrinsic, 0x-hex: what travels in the message.
    pub bytes: String,
    pub from: String,
    pub to: String,
    /// Decimal rao.
    pub rao: String,
    pub tx: String,
    /// The payer's account nonce it spends.
    pub nonce: u64,
    /// When it stops being cashable, unix seconds (estimated from block time).
    pub expires_at: u64,
}

fn snapshot(json: &str) -> Result<Snapshot, WalletError> {
    serde_json::from_str(json).map_err(|_| WalletError::Invalid("no chain snapshot".into()))
}

fn cheque(bytes: Vec<u8>, c: chain::Cheque) -> WalletCheque {
    WalletCheque {
        bytes: format!("0x{}", hex::encode(bytes)),
        from: c.from,
        to: c.to,
        rao: c.rao.to_string(),
        tx: c.tx,
        nonce: c.nonce,
        expires_at: c.expires_at,
    }
}

/// A cheque from a message: hex, bounded before decoding (the wallet bounds it again).
fn unhex(s: &str) -> Result<Vec<u8>, WalletError> {
    let s = s.trim().trim_start_matches("0x");
    (s.len() <= 4096)
        .then(|| hex::decode(s).ok())
        .flatten()
        .ok_or_else(|| WalletError::Invalid("not a cheque".into()))
}

/// Sign a cheque with no network, at `nonce` (the app hands them out: the chain's next nonce,
/// then the lowest number no outstanding cheque of this address holds).
#[uniffi::export]
pub fn wallet_sign_cheque(
    phrase: String,
    snapshot_json: String,
    to: String,
    amount: String,
    nonce: u64,
) -> Result<WalletCheque, WalletError> {
    let rao = arbos_wallet::parse_tao(&amount)?;
    let (bytes, c) = chain::sign(&snapshot(&snapshot_json)?, &seed(&phrase)?, &to, rao, nonce)?;
    Ok(cheque(bytes, c))
}

/// Read a cheque from a message: its signature is verified. Show only what this returns, never
/// the message's own text, and never as paid: only cashing moves money.
#[uniffi::export]
pub fn wallet_read_cheque(
    snapshot_json: String,
    bytes: String,
) -> Result<WalletCheque, WalletError> {
    let raw = unhex(&bytes)?;
    let c = chain::inspect(&snapshot(&snapshot_json)?, &raw)?;
    Ok(cheque(raw, c))
}

/// What became of a cheque handed to the chain.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum WalletCashed {
    /// It succeeded in this finalized block.
    Paid { block: String },
    /// Submitted, not seen finalized yet: not a bounce. Call again later with `since`.
    Pending { since: u64 },
}

/// Cash a cheque (either side may). `since` is what an earlier `Pending` returned for it. An
/// error is a real refusal, in words for the person.
#[uniffi::export(async_runtime = "tokio")]
pub async fn wallet_cash_cheque(
    bytes: String,
    since: Option<u64>,
) -> Result<WalletCashed, WalletError> {
    Ok(
        match chain::cash(Net::Main, &unhex(&bytes)?, since).await? {
            chain::Cashed::Paid(receipt) => WalletCashed::Paid {
                block: receipt.block,
            },
            chain::Cashed::Pending { since } => WalletCashed::Pending { since },
        },
    )
}
