//! Registering a miner on a Bittensor subnet with the user's own wallet.
//!
//! The agent never holds the user's key or their TAO. The `subnet_register` tool posts a
//! money card (`payload.kind = subnet_register`) that names the subnet and the miner's hotkey
//! and waits. The user's iPhone reads the fee, shows the most that can leave the wallet,
//! builds the `register_limit` transaction itself from the card's two fields, signs it and
//! follows it to a finalized block, then answers the card with
//! `approve { registration: { netuid, hotkey, limit_rao, block } }`.
//!
//! Nothing here is signed or submitted by the box, and no bytes from the agent are signed by
//! the phone. An approval that carries no registration (the Mac, an older app) spent nothing
//! and is reported as that.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use tokio::sync::oneshot;

use arbos_core::approval::{ApprovalOutcome, ApprovalRequest, Risk};

use crate::agent::Agent;
use crate::approval::WaitDelivery;
use crate::mcp::{err_result, text_result};
use crate::vault::wait_budget;

/// `payload.kind` of the registration card.
pub const KIND: &str = "subnet_register";

/// Card answers that carried a registration: approval id → what the phone says it did.
fn answers() -> &'static Mutex<BTreeMap<String, Value>> {
    static S: OnceLock<Mutex<BTreeMap<String, Value>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// An `approve` command's payload named a registration: keep the four known fields for the
/// tool waiting on that card. Called before the waiter is woken.
pub fn note_card_answer(approval_id: &str, payload: &Value) {
    if let Some(r) = payload.get("registration").filter(|r| r.is_object()) {
        let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        let clean = json!({
            "netuid": r.get("netuid").and_then(Value::as_u64),
            "hotkey": text("hotkey"),
            "limit_rao": text("limit_rao"),
            "block": text("block"),
        });
        answers()
            .lock()
            .expect("registration answers")
            .insert(approval_id.to_owned(), clean);
    }
}

fn take_card_answer(approval_id: &str) -> Option<Value> {
    answers()
        .lock()
        .expect("registration answers")
        .remove(approval_id)
}

pub fn tool_specs() -> Vec<Value> {
    vec![json!({ "name": "subnet_register",
        "description": "Register a miner on a Bittensor subnet, paid from the user's own wallet. Posts a card on their iPhone that shows the fee right now and the most that can leave their wallet; they approve and their phone signs and sends it — you never hold their key or their TAO, and you cannot pay it any other way. netuid = the subnet number. hotkey = the miner's hotkey address (ss58, starts with 5), made on the mining machine; never ask for its seed or mnemonic. reason = one sentence shown on the card. Waits for the answer and returns whether it registered. The fee is spent, not refunded: call this only after the user said in this chat to register on that subnet.",
        "inputSchema": { "type": "object", "properties": {
            "netuid": { "type": "integer", "minimum": 0, "maximum": 65535 },
            "hotkey": { "type": "string", "description": "the miner's ss58 hotkey address" },
            "reason": { "type": "string", "description": "one sentence shown on the card" } },
            "required": ["netuid", "hotkey", "reason"] } })]
}

/// The shape of an ss58 address: base58 text of the length a 32-byte key encodes to. The
/// phone decodes it for real (checksum and all) before it signs; this only keeps a card for
/// an obvious non-address from being posted.
fn looks_like_ss58(s: &str) -> bool {
    (47..=48).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l'))
}

/// What the tool was asked to do, or why it will not.
fn request(args: &Value) -> Result<(u16, String, String), &'static str> {
    let netuid = args["netuid"]
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .ok_or("netuid must be a subnet number from 0 to 65535")?;
    let hotkey = args["hotkey"].as_str().unwrap_or("").trim().to_owned();
    if !looks_like_ss58(&hotkey) {
        return Err("hotkey must be the miner's ss58 address (48 characters, starts with 5)");
    }
    let reason: String = args["reason"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect();
    if reason.is_empty() {
        return Err("reason is required: one sentence shown on the card");
    }
    Ok((netuid, hotkey, reason))
}

/// True when the phone's answer is for exactly the subnet and hotkey that were asked for.
fn matches(answer: &Value, netuid: u16, hotkey: &str) -> bool {
    answer["netuid"].as_u64() == Some(u64::from(netuid))
        && answer["hotkey"].as_str() == Some(hotkey)
}

/// The `subnet_register` tool: post the card, wait for the phone to sign and send, report.
pub async fn subnet_register(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let (netuid, hotkey, reason) = match request(args) {
        Ok(r) => r,
        Err(why) => return err_result(why),
    };
    let agent_name = agent.cfg.project_name.clone();
    let (tx, rx) = oneshot::channel();
    let req = ApprovalRequest {
        title: format!("Register a miner on subnet {netuid}"),
        summary: reason.clone(),
        risk: Risk::Money,
        payload: json!({ "kind": KIND, "netuid": netuid, "hotkey": hotkey, "reason": reason,
            "agent": agent_name, "projectId": agent.cfg.project_id, "chatId": chat_id,
            "answers": [ { "id": "approve", "label": "Register" },
                         { "id": "deny", "label": "Not now" } ] }),
    };
    let approval_id = match agent
        .begin_approval(chat_id, req, WaitDelivery::Inline(tx))
        .await
    {
        Ok(id) => id,
        Err(e) => return err_result(format!("the card could not be posted: {e}")),
    };
    let outcome = match tokio::time::timeout(wait_budget(), rx).await {
        Ok(Ok(o)) => Some(o),
        Ok(Err(_)) => None,
        Err(_) => {
            agent.park_approval(&approval_id).await;
            None
        }
    };
    let answer = take_card_answer(&approval_id);
    let said = match (outcome, answer) {
        (None, _) => json!({ "registered": false, "waiting": true,
            "reason": "the card is still open on the user's phone. Do not ask again: say in one sentence that it is waiting on them" }),
        (Some(ApprovalOutcome::Approved), Some(a)) if matches(&a, netuid, &hotkey) => {
            agent.append_audit(
                "subnet_register",
                json!({ "netuid": netuid, "hotkey": hotkey, "limit_rao": a["limit_rao"],
                    "block": a["block"], "chat_id": chat_id, "approval_id": approval_id }),
            );
            json!({ "registered": true, "netuid": netuid, "hotkey": hotkey,
                "limit_rao": a["limit_rao"], "block": a["block"],
                "how": "The user's phone followed it into a finalized block. Confirm the slot from the mining machine before you start the miner, and tell the user it is done." })
        }
        (Some(ApprovalOutcome::Approved), _) => json!({ "registered": false,
            "reason": "the card was approved on a device that cannot sign (the Mac app, or an older iPhone app), so nothing was sent and nothing was spent. Ask the user to approve it on their iPhone, then call this again" }),
        (Some(ApprovalOutcome::Rejected { .. }), _) => {
            json!({ "registered": false, "declined": true,
            "reason": "the user said not now; nothing was spent. Do not ask again unless they bring it up" })
        }
        (Some(ApprovalOutcome::Cancelled), _) => json!({ "registered": false, "cancelled": true,
            "reason": "the work was cancelled, so the card was closed; nothing was spent" }),
    };
    text_result(said.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOTKEY: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

    #[test]
    fn a_request_needs_a_subnet_an_address_and_a_reason() {
        let ok = json!({ "netuid": 51, "hotkey": HOTKEY, "reason": "mine with the home GPUs" });
        assert_eq!(request(&ok).unwrap().0, 51);
        for bad in [
            json!({ "netuid": 70000, "hotkey": HOTKEY, "reason": "r" }),
            json!({ "netuid": -1, "hotkey": HOTKEY, "reason": "r" }),
            json!({ "netuid": "51", "hotkey": HOTKEY, "reason": "r" }),
            json!({ "netuid": 51, "hotkey": "5Grw", "reason": "r" }),
            json!({ "netuid": 51, "hotkey": format!("{}0", &HOTKEY[..47]), "reason": "r" }),
            json!({ "netuid": 51, "hotkey": HOTKEY, "reason": "  " }),
        ] {
            assert!(request(&bad).is_err(), "{bad}");
        }
    }

    /// The phone's word counts only for the subnet and hotkey the card asked about, and only
    /// the four known fields of its answer are kept.
    #[test]
    fn an_answer_for_another_subnet_or_hotkey_is_not_a_registration() {
        note_card_answer(
            "a1",
            &json!({ "registration": { "netuid": 51, "hotkey": HOTKEY, "limit_rao": "550",
                "block": "0xab", "extra": "dropped" } }),
        );
        let a = take_card_answer("a1").expect("kept");
        assert!(a.get("extra").is_none());
        assert!(matches(&a, 51, HOTKEY));
        assert!(!matches(&a, 52, HOTKEY));
        assert!(!matches(&a, 51, &HOTKEY.replace('G', "H")));
        assert!(take_card_answer("a1").is_none(), "an answer is used once");
        note_card_answer("a2", &json!({ "registration": "yes" }));
        assert!(take_card_answer("a2").is_none());
    }
}
