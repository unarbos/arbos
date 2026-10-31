//! The relay's view of the homeserver: Matrix client-server calls made as an appservice
//! (`Authorization: Bearer <as_token>`, `?user_id=` masquerading, `?ts=` timestamp massaging for
//! backfill). Only what stages A–C need; no sync loop, no crypto.

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use super::Config;

/// A Matrix error the caller may want to act on (`M_FORBIDDEN`, `M_USER_IN_USE`, …).
#[derive(Debug, thiserror::Error)]
pub enum MatrixError {
    #[error("{status} {errcode}: {error}")]
    Api {
        status: StatusCode,
        errcode: String,
        error: String,
    },
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("bad response: {0}")]
    Shape(String),
}

impl MatrixError {
    pub fn errcode(&self) -> &str {
        match self {
            Self::Api { errcode, .. } => errcode,
            Self::Transport(_) | Self::Shape(_) => "",
        }
    }

    /// Worth retrying later (the homeserver is down or busy) — as opposed to a request Synapse
    /// rejected for a reason that will not change.
    pub fn transient(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Api {
                status, errcode, ..
            } => {
                status.is_server_error()
                    || *status == StatusCode::TOO_MANY_REQUESTS
                    || errcode == "M_LIMIT_EXCEEDED"
            }
            Self::Shape(_) => false,
        }
    }
}

pub type MxResult<T> = Result<T, MatrixError>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    cfg: Config,
}

fn enc(s: &str) -> String {
    // Room ids (`!abc:server`), event ids (`$…`) and user ids (`@…:…`) go in path segments.
    url_escape(s)
}

fn url_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Client {
    pub fn new(http: reqwest::Client, cfg: Config) -> Self {
        Self { http, cfg }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        as_user: Option<&str>,
        ts: Option<i64>,
        body: Option<&Value>,
    ) -> MxResult<Value> {
        let token = match as_user {
            Some(u) => self.cfg.as_token_for(u).to_owned(),
            None => self.cfg.as_token_users.clone(),
        };
        let mut req = self
            .http
            .request(method, format!("{}{}", self.cfg.hs_url, path))
            .bearer_auth(token);
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(u) = as_user {
            q.push(("user_id", u.to_owned()));
        }
        if let Some(ts) = ts {
            q.push(("ts", ts.to_string()));
        }
        if !q.is_empty() {
            req = req.query(&q);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        let v: Value = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text)
                .map_err(|e| MatrixError::Shape(format!("{e}: {}", &text[..text.len().min(200)])))?
        };
        if status.is_success() {
            Ok(v)
        } else {
            Err(MatrixError::Api {
                status,
                errcode: v["errcode"].as_str().unwrap_or("").to_owned(),
                error: v["error"]
                    .as_str()
                    .unwrap_or(&text)
                    .chars()
                    .take(300)
                    .collect(),
            })
        }
    }

    /// `GET /_matrix/client/versions` — reachability check.
    pub async fn versions(&self) -> MxResult<Vec<String>> {
        let v = self
            .call(Method::GET, "/_matrix/client/versions", None, None, None)
            .await?;
        Ok(v["versions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Create the appservice user with this localpart; already existing is fine.
    pub async fn register(&self, mxid: &str) -> MxResult<()> {
        let local = mxid
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or_default()
            .to_owned();
        let body = json!({ "type": "m.login.application_service", "username": local, "inhibit_login": true });
        match self
            .call(
                Method::POST,
                "/_matrix/client/v3/register",
                Some(mxid),
                None,
                Some(&body),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.errcode() == "M_USER_IN_USE" => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Mint a real access token + device for an app device (`m.login.application_service`;
    /// the users registration has no MSC4190). Returns `(access_token, matrix device id)`.
    pub async fn login_appservice(
        &self,
        mxid: &str,
        device_id: &str,
    ) -> MxResult<(String, String)> {
        let body = json!({
            "type": "m.login.application_service",
            "identifier": { "type": "m.id.user", "user": mxid },
            "device_id": device_id,
            "initial_device_display_name": "Arbos",
        });
        let v = self
            .call(
                Method::POST,
                "/_matrix/client/v3/login",
                Some(mxid),
                None,
                Some(&body),
            )
            .await?;
        match (v["access_token"].as_str(), v["device_id"].as_str()) {
            (Some(t), Some(d)) => Ok((t.to_owned(), d.to_owned())),
            _ => Err(MatrixError::Shape("login without access_token".into())),
        }
    }

    /// `POST /logout` with a device's own token (not the appservice token).
    pub async fn logout_token(&self, token: &str) -> MxResult<()> {
        let resp = self
            .http
            .post(format!("{}/_matrix/client/v3/logout", self.cfg.hs_url))
            .bearer_auth(token)
            .json(&json!({}))
            .send()
            .await?;
        if resp.status().is_success() || resp.status() == StatusCode::UNAUTHORIZED {
            Ok(())
        } else {
            Err(MatrixError::Api {
                status: resp.status(),
                errcode: String::new(),
                error: "logout failed".into(),
            })
        }
    }

    pub async fn set_displayname(&self, mxid: &str, name: &str) -> MxResult<()> {
        let path = format!("/_matrix/client/v3/profile/{}/displayname", enc(mxid));
        self.call(
            Method::PUT,
            &path,
            Some(mxid),
            None,
            Some(&json!({ "displayname": name })),
        )
        .await?;
        Ok(())
    }

    /// `POST /createRoom` as `creator`; returns the room id.
    pub async fn create_room(&self, creator: &str, body: &Value) -> MxResult<String> {
        let v = self
            .call(
                Method::POST,
                "/_matrix/client/v3/createRoom",
                Some(creator),
                None,
                Some(body),
            )
            .await?;
        v["room_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| MatrixError::Shape("createRoom without room_id".into()))
    }

    pub async fn invite(&self, room_id: &str, by: &str, who: &str) -> MxResult<()> {
        let path = format!("/_matrix/client/v3/rooms/{}/invite", enc(room_id));
        match self
            .call(
                Method::POST,
                &path,
                Some(by),
                None,
                Some(&json!({ "user_id": who })),
            )
            .await
        {
            Ok(_) => Ok(()),
            // Already in the room (or already invited): nothing to do.
            Err(MatrixError::Api { status, .. }) if status == StatusCode::FORBIDDEN => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn join(&self, room_id: &str, who: &str) -> MxResult<()> {
        let path = format!("/_matrix/client/v3/join/{}", enc(room_id));
        self.call(Method::POST, &path, Some(who), None, Some(&json!({})))
            .await?;
        Ok(())
    }

    pub async fn leave(&self, room_id: &str, who: &str) -> MxResult<()> {
        let path = format!("/_matrix/client/v3/rooms/{}/leave", enc(room_id));
        match self
            .call(Method::POST, &path, Some(who), None, Some(&json!({})))
            .await
        {
            Ok(_) => Ok(()),
            Err(MatrixError::Api { status, .. }) if status == StatusCode::FORBIDDEN => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn kick(&self, room_id: &str, by: &str, who: &str, reason: &str) -> MxResult<()> {
        let path = format!("/_matrix/client/v3/rooms/{}/kick", enc(room_id));
        match self
            .call(
                Method::POST,
                &path,
                Some(by),
                None,
                Some(&json!({ "user_id": who, "reason": reason })),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(MatrixError::Api { status, .. }) if status == StatusCode::FORBIDDEN => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// `PUT /rooms/{id}/send/{type}/{txn}`; returns the event id.
    pub async fn send(
        &self,
        room_id: &str,
        sender: &str,
        event_type: &str,
        txn_id: &str,
        content: &Value,
        ts: Option<i64>,
    ) -> MxResult<String> {
        let path = format!(
            "/_matrix/client/v3/rooms/{}/send/{}/{}",
            enc(room_id),
            enc(event_type),
            enc(txn_id)
        );
        let v = self
            .call(Method::PUT, &path, Some(sender), ts, Some(content))
            .await?;
        v["event_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| MatrixError::Shape("send without event_id".into()))
    }

    pub async fn send_state(
        &self,
        room_id: &str,
        sender: &str,
        event_type: &str,
        state_key: &str,
        content: &Value,
    ) -> MxResult<String> {
        let path = format!(
            "/_matrix/client/v3/rooms/{}/state/{}/{}",
            enc(room_id),
            enc(event_type),
            enc(state_key)
        );
        let v = self
            .call(Method::PUT, &path, Some(sender), None, Some(content))
            .await?;
        Ok(v["event_id"].as_str().unwrap_or_default().to_owned())
    }

    pub async fn state(
        &self,
        room_id: &str,
        as_user: &str,
        event_type: &str,
        state_key: &str,
    ) -> MxResult<Option<Value>> {
        let path = format!(
            "/_matrix/client/v3/rooms/{}/state/{}/{}",
            enc(room_id),
            enc(event_type),
            enc(state_key)
        );
        match self
            .call(Method::GET, &path, Some(as_user), None, None)
            .await
        {
            Ok(v) => Ok(Some(v)),
            Err(MatrixError::Api { status, .. }) if status == StatusCode::NOT_FOUND => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every current state event of a room (`type`, `state_key`, `sender`, `content`), as seen by
    /// `as_user`. Unlike [`Self::state`] this keeps the events' senders, which room v12 needs: the
    /// create event's sender *is* the creator there (MSC4291 dropped `content.creator`).
    pub async fn room_state(&self, room_id: &str, as_user: &str) -> MxResult<Vec<Value>> {
        let path = format!("/_matrix/client/v3/rooms/{}/state", enc(room_id));
        let v = self
            .call(Method::GET, &path, Some(as_user), None, None)
            .await?;
        v.as_array()
            .cloned()
            .ok_or_else(|| MatrixError::Shape("room state is not an array".into()))
    }

    /// Joined members of a room, as seen by `as_user` (who must be in it).
    pub async fn joined_members(&self, room_id: &str, as_user: &str) -> MxResult<Vec<String>> {
        let path = format!("/_matrix/client/v3/rooms/{}/joined_members", enc(room_id));
        let v = self
            .call(Method::GET, &path, Some(as_user), None, None)
            .await?;
        Ok(v["joined"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default())
    }

    /// Newest `limit` events of a room, newest first (`dir=b`).
    pub async fn messages_back(
        &self,
        room_id: &str,
        as_user: &str,
        limit: usize,
    ) -> MxResult<Vec<Value>> {
        let path = format!(
            "/_matrix/client/v3/rooms/{}/messages?dir=b&limit={limit}",
            enc(room_id)
        );
        let v = self
            .call(Method::GET, &path, Some(as_user), None, None)
            .await?;
        Ok(v["chunk"].as_array().cloned().unwrap_or_default())
    }

    /// One event, as seen by `as_user` (who must be in the room).
    pub async fn event(&self, room_id: &str, as_user: &str, event_id: &str) -> MxResult<Value> {
        let path = format!(
            "/_matrix/client/v3/rooms/{}/event/{}",
            enc(room_id),
            enc(event_id)
        );
        self.call(Method::GET, &path, Some(as_user), None, None)
            .await
    }

    /// `POST /pushers/set` as the user (register or remove the relay's push gateway).
    pub async fn set_pusher(&self, who: &str, body: &Value) -> MxResult<()> {
        self.call(
            Method::POST,
            "/_matrix/client/v3/pushers/set",
            Some(who),
            None,
            Some(body),
        )
        .await?;
        Ok(())
    }

    /// `m.read` receipt (private when `private` is set).
    pub async fn read_receipt(
        &self,
        room_id: &str,
        who: &str,
        event_id: &str,
        private: bool,
    ) -> MxResult<()> {
        let kind = if private { "m.read.private" } else { "m.read" };
        let path = format!(
            "/_matrix/client/v3/rooms/{}/receipt/{kind}/{}",
            enc(room_id),
            enc(event_id)
        );
        self.call(Method::POST, &path, Some(who), None, Some(&json!({})))
            .await?;
        Ok(())
    }

    /// Room tag on or off for one user (`m.lowpriority` = archived).
    pub async fn set_tag(&self, room_id: &str, who: &str, tag: &str, on: bool) -> MxResult<()> {
        let path = format!(
            "/_matrix/client/v3/user/{}/rooms/{}/tags/{}",
            enc(who),
            enc(room_id),
            enc(tag)
        );
        let r = if on {
            self.call(
                Method::PUT,
                &path,
                Some(who),
                None,
                Some(&json!({ "order": 0.5 })),
            )
            .await
        } else {
            self.call(Method::DELETE, &path, Some(who), None, None)
                .await
        };
        match r {
            Ok(_) => Ok(()),
            Err(MatrixError::Api { status, .. }) if status == StatusCode::NOT_FOUND && !on => {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Mark a DM in both users' `m.direct` account data.
    pub async fn set_direct(&self, who: &str, other: &str, room_id: &str) -> MxResult<()> {
        let path = format!("/_matrix/client/v3/user/{}/account_data/m.direct", enc(who));
        let mut current = match self.call(Method::GET, &path, Some(who), None, None).await {
            Ok(v) => v,
            Err(MatrixError::Api { status, .. }) if status == StatusCode::NOT_FOUND => json!({}),
            Err(e) => return Err(e),
        };
        let list = current
            .as_object_mut()
            .map(|m| m.entry(other.to_owned()).or_insert_with(|| json!([])))
            .and_then(Value::as_array_mut);
        if let Some(list) = list {
            if !list.iter().any(|r| r.as_str() == Some(room_id)) {
                list.push(json!(room_id));
            }
        }
        self.call(Method::PUT, &path, Some(who), None, Some(&current))
            .await?;
        Ok(())
    }

    /// Unread counts per joined room as the homeserver computes them (stage B shadow-compare):
    /// one initial `/sync` with an empty timeline, lazy-loaded members.
    pub async fn unread_counts(&self, who: &str) -> MxResult<Vec<(String, i64, i64)>> {
        // One timeline event per room: with an empty timeline Synapse leaves the room out of
        // an initial sync altogether, counts included.
        let filter = json!({
            "room": {
                "timeline": { "limit": 1, "lazy_load_members": true },
                "state": { "lazy_load_members": true, "types": ["m.room.create"] },
                "ephemeral": { "types": [] }
            },
            "presence": { "types": [] }, "account_data": { "types": [] }
        });
        let path = format!(
            "/_matrix/client/v3/sync?timeout=0&filter={}",
            url_escape(&filter.to_string())
        );
        let v = self.call(Method::GET, &path, Some(who), None, None).await?;
        let mut out = Vec::new();
        if let Some(rooms) = v["rooms"]["join"].as_object() {
            for (room_id, r) in rooms {
                let n = &r["unread_notifications"];
                out.push((
                    room_id.clone(),
                    n["notification_count"].as_i64().unwrap_or(0),
                    n["highlight_count"].as_i64().unwrap_or(0),
                ));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_matrix_ids_for_paths() {
        assert_eq!(enc("!abc:arbos.life"), "%21abc%3Aarbos.life");
        assert_eq!(enc("@gh_1:arbos.life"), "%40gh_1%3Aarbos.life");
        assert_eq!(enc("$ev/x+y"), "%24ev%2Fx%2By");
        assert_eq!(enc("m.room.message"), "m.room.message");
    }

    #[test]
    fn transient_errors() {
        let e = MatrixError::Api {
            status: StatusCode::BAD_GATEWAY,
            errcode: "M_UNKNOWN".into(),
            error: String::new(),
        };
        assert!(e.transient());
        let e = MatrixError::Api {
            status: StatusCode::FORBIDDEN,
            errcode: "M_FORBIDDEN".into(),
            error: String::new(),
        };
        assert!(!e.transient());
        let e = MatrixError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            errcode: "M_LIMIT_EXCEEDED".into(),
            error: String::new(),
        };
        assert!(e.transient());
    }
}
