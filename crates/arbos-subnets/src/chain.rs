//! The subnets the chain lists, with the repository each one names for itself. Plain JSON-RPC
//! over HTTPS, read only; nothing here signs or holds a key.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub struct Subnet {
    pub netuid: u16,
    pub name: String,
    pub repo: String,
}

fn twox128(s: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    for seed in 0..2u64 {
        let h = twox_hash::XxHash64::oneshot(seed, s.as_bytes()).to_le_bytes();
        out[seed as usize * 8..][..8].copy_from_slice(&h);
    }
    out
}

/// `SubtensorModule.SubnetIdentitiesV3`: netuid → identity. A dissolved subnet's row is removed
/// by the chain, which is what prunes it here.
fn prefix() -> String {
    format!(
        "0x{}{}",
        hex::encode(twox128("SubtensorModule")),
        hex::encode(twox128("SubnetIdentitiesV3"))
    )
}

async fn rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let mut v: Value = http
        .post(url)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        bail!("{method}: {e}");
    }
    Ok(v["result"].take())
}

/// One SCALE `Vec<u8>`: a compact length, then that many bytes.
fn take_bytes(input: &mut &[u8]) -> Option<Vec<u8>> {
    let b = *input;
    let (len, used): (usize, usize) = match *b.first()? & 3 {
        0 => (b[0] as usize >> 2, 1),
        1 => (
            u16::from_le_bytes(b.get(..2)?.try_into().ok()?) as usize >> 2,
            2,
        ),
        2 => (
            u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize >> 2,
            4,
        ),
        _ => return None, // big-integer lengths: no identity field is 1 GiB
    };
    let out = b.get(used..used.checked_add(len)?)?.to_vec();
    *input = &b[used + len..];
    Some(out)
}

/// The identity's first two fields are `subnet_name` and `github_repo`; the rest is not read.
fn decode(key: &str, value: &str) -> Option<Subnet> {
    let key = hex::decode(key.strip_prefix("0x")?).ok()?;
    // The netuid is the key's last two bytes whichever hasher prefixes it.
    let netuid = u16::from_le_bytes(key.get(key.len().checked_sub(2)?..)?.try_into().ok()?);
    let value = hex::decode(value.strip_prefix("0x")?).ok()?;
    let mut rest = value.as_slice();
    let name = String::from_utf8_lossy(&take_bytes(&mut rest)?).into_owned();
    let repo = String::from_utf8_lossy(&take_bytes(&mut rest)?).into_owned();
    Some(Subnet { netuid, name, repo })
}

pub async fn subnets(http: &reqwest::Client, url: &str) -> Result<Vec<Subnet>> {
    const PAGE: usize = 1000;
    let prefix = prefix();
    let mut keys: Vec<String> = Vec::new();
    loop {
        let page = rpc(
            http,
            url,
            "state_getKeysPaged",
            json!([prefix, PAGE, keys.last()]),
        )
        .await?;
        let page: Vec<String> = serde_json::from_value(page).context("keys")?;
        let n = page.len();
        keys.extend(page);
        if n < PAGE {
            break;
        }
    }
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let got = rpc(http, url, "state_queryStorageAt", json!([keys])).await?;
    let changes = got[0]["changes"]
        .as_array()
        .ok_or_else(|| anyhow!("state_queryStorageAt: no changes"))?;
    let mut out: Vec<Subnet> = changes
        .iter()
        .filter_map(|c| decode(c[0].as_str()?, c[1].as_str()?))
        .collect();
    out.sort_by_key(|s| s.netuid);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_prefix_is_the_pallets() {
        // twox128("SubtensorModule") as every Subtensor storage key begins.
        assert!(prefix().starts_with("0x658faa385070e074c85bf6b568cf0555"));
    }

    #[test]
    fn an_identity_decodes_and_a_short_one_does_not() {
        // "ab" (len 2 → 0x08), then a 70-byte repo (two-byte length: 70 << 2 | 1 = 0x0119).
        let repo = "r".repeat(70);
        let value = format!("0x08{}1901{}ff", hex::encode("ab"), hex::encode(&repo));
        let key = format!("{}4000", prefix());
        assert_eq!(
            decode(&key, &value),
            Some(Subnet {
                netuid: 64,
                name: "ab".into(),
                repo
            })
        );
        assert_eq!(decode(&key, "0x08ab"), None);
        assert_eq!(decode(&key, "0x"), None);
    }
}
