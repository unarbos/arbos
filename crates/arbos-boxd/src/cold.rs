//! The cold tier's object store (docs/box-scaling.md §3.3, Phase 3): an S3-compatible bucket
//! (Cloudflare R2) reached with SigV4, the relay's own pattern in `arbos-relay/src/sites.rs`.
//!
//! A file goes up as numbered chunks, `boxes/<user_id>/<part>/<n>`, and a [`Manifest`] of
//! their SHA-256s. Nothing here trusts the store: an upload counts only once every chunk has
//! been read back and hashed, and a download writes only chunks whose hash is the manifest's.
//!
//! Off unless `ARBOS_BOXD_S3_ENDPOINT` is set. One bucket, one host.

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use hmac::{Hmac, Mac};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One object. Small enough to hold in memory and to send again when a request fails; a
/// 16 GiB checkpoint is 256 of them, far under a single PUT's 5 GiB limit.
const CHUNK: usize = 64 << 20;
/// A request is tried this many times when the store does not answer or answers 5xx or 429.
const TRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// More chunks than a 128 GiB disk that did not compress at all.
const MAX_CHUNKS: usize = 4096;

/// What was uploaded: the record a download is checked against. The daemon keeps its own copy
/// on the host (`boxes.rs`), and that copy is the one it trusts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    v: u32,
    /// The length of the packed file.
    pub bytes: u64,
    chunk_bytes: u64,
    /// Lowercase hex SHA-256 of each chunk, in order.
    chunks: Vec<String>,
    /// The length of what was packed: a `work.img`'s apparent size, or 0 for a checkpoint.
    pub source_bytes: u64,
}

impl Manifest {
    /// A manifest read from disk, or `None` if it is not one this daemon wrote.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let m: Self = serde_json::from_slice(bytes).ok()?;
        let hex =
            |s: &String| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        let ok = m.v == 1
            && m.chunk_bytes > 0
            && m.chunks.len() <= MAX_CHUNKS
            && m.chunks.len() as u64 == m.bytes.div_ceil(m.chunk_bytes)
            && m.chunks.iter().all(hex);
        ok.then_some(m)
    }

    /// The length of chunk `i`: every one is full but the last.
    fn len(&self, i: usize) -> usize {
        (self.bytes - i as u64 * self.chunk_bytes).min(self.chunk_bytes) as usize
    }
}

pub struct Store {
    /// `https://<host>[:<port>]`, no trailing slash.
    endpoint: String,
    host: String,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    http: reqwest::Client,
    chunk: usize,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// The two lines of a credentials file: the access key id, then the secret. The file must be
/// a plain file nobody but its owner can read, as the daemon's token file is.
fn read_credentials(path: &str) -> Result<(String, String)> {
    let md = std::fs::symlink_metadata(path).with_context(|| format!("credentials file {path}"))?;
    if !md.is_file() || md.mode() & 0o077 != 0 {
        bail!("credentials file {path} must be a plain file with mode 0600");
    }
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    match (lines.next(), lines.next(), lines.next()) {
        (Some(id), Some(secret), None) => Ok((id.to_owned(), secret.to_owned())),
        _ => bail!("credentials file {path} must hold two lines: the access key id and the secret"),
    }
}

impl Store {
    /// `ARBOS_BOXD_S3_ENDPOINT`, `_BUCKET`, `_REGION` (default `auto`) and the credentials:
    /// `_CREDENTIALS_FILE`, or `_ACCESS_KEY_ID` and `_SECRET_ACCESS_KEY`. No endpoint: the cold
    /// tier is off. An endpoint without the rest stops the daemon rather than run a tier that
    /// cannot work.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |k: &str| {
            std::env::var(format!("ARBOS_BOXD_S3_{k}"))
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let Some(endpoint) = var("ENDPOINT") else {
            return Ok(None);
        };
        let bucket = var("BUCKET")
            .context("ARBOS_BOXD_S3_ENDPOINT is set; ARBOS_BOXD_S3_BUCKET must name the bucket")?;
        let (access_key, secret_key) = match var("CREDENTIALS_FILE") {
            Some(file) => read_credentials(&file)?,
            None => var("ACCESS_KEY_ID")
                .zip(var("SECRET_ACCESS_KEY"))
                .context("ARBOS_BOXD_S3_ENDPOINT is set; ARBOS_BOXD_S3_CREDENTIALS_FILE, or ARBOS_BOXD_S3_ACCESS_KEY_ID and ARBOS_BOXD_S3_SECRET_ACCESS_KEY, must be too")?,
        };
        let region = var("REGION").unwrap_or_else(|| "auto".into());
        Self::new(&endpoint, &bucket, &region, access_key, secret_key).map(Some)
    }

    /// The endpoint is `https://host[:port]` and nothing more; plain HTTP only on loopback,
    /// where the tests' store is. A user's disk does not cross a network in the clear.
    fn new(
        endpoint: &str,
        bucket: &str,
        region: &str,
        access_key: String,
        secret_key: String,
    ) -> Result<Self> {
        let url = reqwest::Url::parse(endpoint).ok();
        let host = url
            .as_ref()
            .filter(|u| {
                let loopback = u
                    .host_str()
                    .and_then(|h| h.parse::<std::net::Ipv4Addr>().ok())
                    .is_some_and(|ip| ip.is_loopback());
                (u.scheme() == "https" || (u.scheme() == "http" && loopback))
                    && u.path() == "/"
                    && u.query().is_none()
                    && u.fragment().is_none()
                    && u.username().is_empty()
                    && u.password().is_none()
            })
            .and_then(|u| {
                Some(match u.port() {
                    Some(port) => format!("{}:{port}", u.host_str()?),
                    None => u.host_str()?.to_owned(),
                })
            })
            .with_context(|| {
                format!("ARBOS_BOXD_S3_ENDPOINT must be https://<host>, not {endpoint}")
            })?;
        let name = |s: &str, max: usize| {
            (1..=max).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        };
        if !name(bucket, 63) || !name(region, 32) {
            bail!("ARBOS_BOXD_S3_BUCKET and ARBOS_BOXD_S3_REGION are lowercase letters, digits, '-' and '.'");
        }
        if access_key.is_empty()
            || secret_key.is_empty()
            || access_key.contains(char::is_whitespace)
        {
            bail!("the object store's access key id or secret is empty");
        }
        Ok(Self {
            endpoint: format!(
                "{}://{host}",
                url.map_or("https".into(), |u| u.scheme().to_owned())
            ),
            host,
            bucket: bucket.to_owned(),
            region: region.to_owned(),
            access_key,
            secret_key,
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            chunk: CHUNK,
        })
    }

    /// The SigV4 `Authorization` of a request with no query, signing the host, the payload
    /// hash and the date. `uri` needs no encoding: a bucket and a key here are letters, digits,
    /// `.`, `-`, `_` and `/`.
    fn authorization(&self, method: &str, uri: &str, payload_hash: &str, amz_date: &str) -> String {
        let date = &amz_date[..8];
        let signed = "host;x-amz-content-sha256;x-amz-date";
        let request = format!(
            "{method}\n{uri}\n\nhost:{}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n\n{signed}\n{payload_hash}",
            self.host
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            sha256_hex(request.as_bytes())
        );
        let mut key = hmac(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        );
        for part in [self.region.as_str(), "s3", "aws4_request"] {
            key = hmac(&key, part.as_bytes());
        }
        let signature = hex::encode(hmac(&key, to_sign.as_bytes()));
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            self.access_key
        )
    }

    /// One request, and at most `max_reply` bytes of its answer.
    async fn once(
        &self,
        method: &Method,
        key: &str,
        body: &[u8],
        max_reply: usize,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let uri = format!("/{}/{key}", self.bucket);
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let payload_hash = sha256_hex(body);
        let mut r = self
            .http
            .request(method.clone(), format!("{}{uri}", self.endpoint))
            .header(
                "authorization",
                self.authorization(method.as_str(), &uri, &payload_hash, &amz_date),
            )
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .body(body.to_vec())
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let status = r.status();
        let mut out = Vec::new();
        // Only a 200 carries anything this reads: an error's body can echo the key id.
        if status == StatusCode::OK && max_reply > 0 {
            while let Some(chunk) = r.chunk().await.map_err(reqwest::Error::without_url)? {
                if out.len() + chunk.len() > max_reply {
                    bail!("the object is longer than the {max_reply} bytes recorded for it");
                }
                out.extend_from_slice(&chunk);
            }
        }
        Ok((status, out))
    }

    /// A request, again while the store is unreachable or says to come back. The error names
    /// the object and the status, never the endpoint, a header or a reply body.
    async fn send(
        &self,
        method: Method,
        key: &str,
        body: &[u8],
        max_reply: usize,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let mut failed = anyhow!("not tried");
        for attempt in 0..TRIES {
            tokio::time::sleep(Duration::from_millis(500) * attempt).await;
            match self.once(&method, key, body, max_reply).await {
                Ok((s, _)) if s.is_server_error() || s == StatusCode::TOO_MANY_REQUESTS => {
                    failed = anyhow!("object store {method} {key}: {s}");
                }
                Ok(done) => return Ok(done),
                Err(e) => failed = anyhow!("object store {method} {key}: {e:#}"),
            }
        }
        Err(failed)
    }

    async fn put(&self, key: &str, body: &[u8]) -> Result<()> {
        let (status, _) = self.send(Method::PUT, key, body, 0).await?;
        if !status.is_success() {
            bail!("object store PUT {key}: {status}");
        }
        Ok(())
    }

    /// The object, if it is exactly `len` bytes that hash to `sha256`.
    async fn get(&self, key: &str, len: usize, sha256: &str) -> Result<Vec<u8>> {
        let (status, body) = self.send(Method::GET, key, &[], len).await?;
        if status != StatusCode::OK {
            bail!("object store GET {key}: {status}");
        }
        if body.len() != len || sha256_hex(&body) != sha256 {
            bail!("object store GET {key}: not the {len} bytes that were uploaded");
        }
        Ok(body)
    }

    fn chunk_key(prefix: &str, i: usize) -> String {
        format!("{prefix}/{i:05}")
    }

    /// Upload `file` under `prefix` and read every chunk of it back. Only then is the manifest
    /// written beside the chunks (`<prefix>.json`, for an operator rebuilding a lost host) and
    /// returned: a manifest in hand is an upload that round-tripped.
    pub async fn upload(&self, prefix: &str, file: &str, source_bytes: u64) -> Result<Manifest> {
        let mut f = std::fs::File::open(file)?;
        let mut m = Manifest {
            v: 1,
            bytes: 0,
            chunk_bytes: self.chunk as u64,
            chunks: Vec::new(),
            source_bytes,
        };
        loop {
            let mut buf = Vec::new();
            (&mut f).take(m.chunk_bytes).read_to_end(&mut buf)?;
            if buf.is_empty() {
                break;
            }
            if m.chunks.len() == MAX_CHUNKS {
                bail!("{file} is more than {MAX_CHUNKS} chunks");
            }
            self.put(&Self::chunk_key(prefix, m.chunks.len()), &buf)
                .await?;
            m.bytes += buf.len() as u64;
            m.chunks.push(sha256_hex(&buf));
        }
        for (i, sha) in m.chunks.iter().enumerate() {
            self.get(&Self::chunk_key(prefix, i), m.len(i), sha).await?;
        }
        self.put(&format!("{prefix}.json"), &serde_json::to_vec(&m)?)
            .await?;
        Ok(m)
    }

    /// Download what `m` records to `file`, which only root reads. Every chunk is checked
    /// before it is written; on any failure the file is removed.
    pub async fn download(&self, prefix: &str, m: &Manifest, file: &str) -> Result<()> {
        let write = async {
            let mut f = std::fs::File::options()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(file)?;
            for (i, sha) in m.chunks.iter().enumerate() {
                f.write_all(&self.get(&Self::chunk_key(prefix, i), m.len(i), sha).await?)?;
            }
            Ok(f.sync_all()?)
        };
        let done: Result<()> = write.await;
        if done.is_err() {
            let _ = std::fs::remove_file(file);
        }
        done
    }

    /// Delete what `m` records. A missing object is as good as a deleted one.
    pub async fn remove(&self, prefix: &str, m: &Manifest) -> Result<()> {
        let keys = (0..m.chunks.len()).map(|i| Self::chunk_key(prefix, i));
        for key in keys.chain([format!("{prefix}.json")]) {
            let (status, _) = self.send(Method::DELETE, &key, &[], 0).await?;
            if !status.is_success() && status != StatusCode::NOT_FOUND {
                bail!("object store DELETE {key}: {status}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::response::{IntoResponse, Response};

    const KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    /// A bucket in memory that answers as S3 does: it takes only a request whose
    /// `Authorization` names the key id and whose payload hash is the body's.
    #[derive(Default)]
    pub struct Fake {
        /// `/<bucket>/<key>` → the object.
        pub objects: Mutex<BTreeMap<String, Vec<u8>>>,
        /// Every request it took: method and path.
        pub calls: Mutex<Vec<String>>,
        /// Answer this status to the next so many requests whose path contains the text.
        pub fail: Mutex<Option<(u16, usize, &'static str)>>,
        /// Keep a PUT whose path contains the text with its first byte changed.
        pub mangle: Mutex<Option<&'static str>>,
    }

    impl Fake {
        pub fn keys(&self) -> Vec<String> {
            self.objects.lock().unwrap().keys().cloned().collect()
        }
        pub fn called(&self, needle: &str) -> usize {
            let calls = self.calls.lock().unwrap();
            calls.iter().filter(|c| c.contains(needle)).count()
        }
    }

    async fn answer(
        State(fake): State<Arc<Fake>>,
        method: Method,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
    ) -> Response {
        let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("");
        let (auth, hash) = (header("authorization"), header("x-amz-content-sha256"));
        let (path, body) = (uri.path().to_owned(), body.to_vec());
        if !auth.starts_with(&format!("AWS4-HMAC-SHA256 Credential={KEY_ID}/"))
            || hash != sha256_hex(&body)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        fake.calls.lock().unwrap().push(format!("{method} {path}"));
        if let Some((status, left, needle)) = fake.fail.lock().unwrap().as_mut() {
            if *left > 0 && path.contains(*needle) {
                *left -= 1;
                // What S3 says of a bad signature names the key id; none of it may be repeated.
                let body = format!("<Error><AWSAccessKeyId>{KEY_ID}</AWSAccessKeyId></Error>");
                return (StatusCode::from_u16(*status).unwrap(), body).into_response();
            }
        }
        let mut objects = fake.objects.lock().unwrap();
        match method.as_str() {
            "PUT" => {
                let mut body = body;
                if fake
                    .mangle
                    .lock()
                    .unwrap()
                    .is_some_and(|m| path.contains(m))
                {
                    body[0] ^= 1;
                }
                objects.insert(path, body);
            }
            "GET" => {
                return objects.get(&path).cloned().map_or(
                    StatusCode::NOT_FOUND.into_response(),
                    IntoResponse::into_response,
                )
            }
            "DELETE" if objects.remove(&path).is_none() => {
                return StatusCode::NOT_FOUND.into_response()
            }
            _ => {}
        }
        StatusCode::OK.into_response()
    }

    /// The fake on a loopback port, and a store for it that cuts a file into 4-byte chunks.
    pub async fn served() -> (Arc<Fake>, Store) {
        let fake = Arc::new(Fake::default());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = axum::Router::new()
            .fallback(answer)
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(l, app).await });
        let mut store = Store::new(&url, "cold", "auto", KEY_ID.into(), SECRET.into()).unwrap();
        store.chunk = 4;
        (fake, store)
    }

    fn scratch(tag: &str) -> String {
        let dir =
            std::env::temp_dir().join(format!("arbos-boxd-cold-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_str().unwrap().to_owned()
    }

    /// The signature of one PUT, against an independent SigV4 that reproduces AWS's published
    /// example (`GET /?lifecycle`, `fea454ca…3543`) from the same key.
    #[test]
    fn a_request_is_signed_as_sigv4_says() {
        let store = Store::new(
            "https://store.example.com",
            "arbos-cold",
            "auto",
            KEY_ID.into(),
            SECRET.into(),
        )
        .unwrap();
        let auth = store.authorization(
            "PUT",
            "/arbos-cold/boxes/u_a/ckpt/00000",
            &sha256_hex(b"cold"),
            "20130524T000000Z",
        );
        assert_eq!(
            auth,
            format!("AWS4-HMAC-SHA256 Credential={KEY_ID}/20130524/auto/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=1383eb79ef1b6b4319cd1d42204b8a374590eede55e5832312fcd66052e607bc")
        );
    }

    /// The endpoint is HTTPS and only a host, the names are names, and the credentials file is
    /// private and holds exactly the two lines.
    #[test]
    fn the_store_is_https_and_its_credentials_file_private() {
        let new = |endpoint: &str, bucket: &str, region: &str| {
            Store::new(endpoint, bucket, region, KEY_ID.into(), SECRET.into())
        };
        for ok in [
            "https://abc.r2.cloudflarestorage.com",
            "https://store.example.com:8443/",
            "http://127.0.0.1:9000",
        ] {
            assert!(new(ok, "arbos-cold", "auto").is_ok(), "{ok}");
        }
        for bad in [
            "http://store.example.com",
            "http://10.0.0.5:9000",
            "store.example.com",
            "ftp://store.example.com",
            "https://store.example.com/bucket",
            "https://store.example.com?x=1",
            "https://id:secret@store.example.com",
            "",
        ] {
            assert!(new(bad, "arbos-cold", "auto").is_err(), "{bad}");
        }
        for (bucket, region) in [
            ("", "auto"),
            ("Cold", "auto"),
            ("a/b", "auto"),
            ("cold", ""),
            ("cold", "us east"),
        ] {
            assert!(
                new("https://store.example.com", bucket, region).is_err(),
                "{bucket:?} {region:?}"
            );
        }
        assert!(Store::new(
            "https://store.example.com",
            "cold",
            "auto",
            KEY_ID.into(),
            String::new()
        )
        .is_err());

        let dir = scratch("creds");
        let path = format!("{dir}/creds");
        let write = |text: &str, mode: u32| {
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        write(&format!("{KEY_ID}\n{SECRET}\n"), 0o600);
        assert_eq!(
            read_credentials(&path).unwrap(),
            (KEY_ID.to_owned(), SECRET.to_owned())
        );
        write(&format!("{KEY_ID}\n{SECRET}\n"), 0o640);
        assert!(read_credentials(&path).is_err(), "group can read it");
        for text in ["", KEY_ID, "a\nb\nc\n"] {
            write(text, 0o600);
            assert!(read_credentials(&path).is_err(), "{text:?}");
        }
        std::os::unix::fs::symlink(&path, format!("{dir}/link")).unwrap();
        assert!(read_credentials(&format!("{dir}/link")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_manifest_this_daemon_wrote_is_read() {
        let sha = "a".repeat(64);
        let m = |v: u32, bytes: u64, chunk: u64, chunks: Vec<&str>| {
            serde_json::to_vec(&serde_json::json!({ "v": v, "bytes": bytes, "chunk_bytes": chunk, "chunks": chunks, "source_bytes": 0 })).unwrap()
        };
        assert!(Manifest::parse(&m(1, 9, 4, vec![&sha, &sha, &sha])).is_some());
        assert!(Manifest::parse(&m(1, 0, 4, vec![])).is_some());
        let upper = "A".repeat(64);
        for bad in [
            m(2, 9, 4, vec![&sha, &sha, &sha]),
            m(1, 9, 4, vec![&sha, &sha]),
            m(1, 9, 0, vec![&sha]),
            m(1, 4, 4, vec!["abc"]),
            m(1, 4, 4, vec![&upper]),
            b"{}".to_vec(),
            br#"{"v":1,"bytes":0,"chunk_bytes":4,"chunks":[],"source_bytes":0,"x":1}"#.to_vec(),
        ] {
            assert!(
                Manifest::parse(&bad).is_none(),
                "{}",
                String::from_utf8_lossy(&bad)
            );
        }
    }

    /// A file goes up in chunks, is read back before it counts, comes down the same, and is
    /// gone after a remove.
    #[tokio::test]
    async fn a_file_round_trips_through_the_store() {
        let (fake, store) = served().await;
        let dir = scratch("round");
        let (src, back) = (format!("{dir}/src"), format!("{dir}/back"));
        std::fs::write(&src, b"0123456789").unwrap();
        let m = store.upload("boxes/u_a/ckpt", &src, 7).await.unwrap();
        assert_eq!((m.bytes, m.chunks.len(), m.source_bytes), (10, 3, 7));
        assert_eq!(
            fake.keys(),
            [
                "/cold/boxes/u_a/ckpt.json",
                "/cold/boxes/u_a/ckpt/00000",
                "/cold/boxes/u_a/ckpt/00001",
                "/cold/boxes/u_a/ckpt/00002"
            ]
        );
        assert_eq!(
            (fake.called("PUT"), fake.called("GET")),
            (4, 3),
            "every chunk was read back"
        );
        let stored = fake.objects.lock().unwrap()["/cold/boxes/u_a/ckpt.json"].clone();
        assert_eq!(Manifest::parse(&stored), Some(m.clone()));

        store.download("boxes/u_a/ckpt", &m, &back).await.unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"0123456789");
        assert_eq!(std::fs::metadata(&back).unwrap().mode() & 0o777, 0o600);
        store.remove("boxes/u_a/ckpt", &m).await.unwrap();
        assert!(fake.keys().is_empty());
        store.remove("boxes/u_a/ckpt", &m).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A chunk that is changed, cut short, grown or gone fails the download and leaves no file;
    /// a store that loses or changes a chunk between the PUT and the read-back fails the upload.
    #[tokio::test]
    async fn a_chunk_that_is_not_what_was_uploaded_is_refused() {
        let (fake, store) = served().await;
        let dir = scratch("corrupt");
        let (src, back) = (format!("{dir}/src"), format!("{dir}/back"));
        std::fs::write(&src, b"0123456789").unwrap();
        let m = store.upload("boxes/u_a/work", &src, 0).await.unwrap();
        let key = "/cold/boxes/u_a/work/00001".to_owned();
        for wrong in [Some(&b"4x67"[..]), Some(b"456"), Some(b"45678"), None] {
            match wrong {
                Some(bytes) => drop(
                    fake.objects
                        .lock()
                        .unwrap()
                        .insert(key.clone(), bytes.to_vec()),
                ),
                None => drop(fake.objects.lock().unwrap().remove(&key)),
            }
            let e = store
                .download("boxes/u_a/work", &m, &back)
                .await
                .unwrap_err();
            assert!(e.to_string().contains("boxes/u_a/work/00001"), "{e}");
            assert!(!std::path::Path::new(&back).exists(), "{wrong:?}");
        }

        // The store keeps something other than it was sent: the read-back fails the upload,
        // and no manifest is written or returned.
        fake.objects.lock().unwrap().clear();
        *fake.mangle.lock().unwrap() = Some("00002");
        let e = store.upload("boxes/u_a/work", &src, 0).await.unwrap_err();
        assert!(e.to_string().contains("boxes/u_a/work/00002"), "{e}");
        assert!(!fake.keys().iter().any(|k| k.ends_with(".json")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 503 is asked again and the transfer goes through; a 403 is not, and what it says
    /// (S3 names the key id in it) is in no error.
    #[tokio::test]
    async fn the_store_is_asked_again_only_when_it_may_answer_differently() {
        let (fake, store) = served().await;
        let dir = scratch("retry");
        let src = format!("{dir}/src");
        std::fs::write(&src, b"0123").unwrap();
        *fake.fail.lock().unwrap() = Some((503, 2, "00000"));
        store.upload("boxes/u_a/ckpt", &src, 0).await.unwrap();
        assert_eq!(fake.called("PUT /cold/boxes/u_a/ckpt/00000"), 3);

        *fake.fail.lock().unwrap() = Some((403, 9, "00000"));
        let before = fake.called("PUT");
        let e = format!(
            "{:#}",
            store.upload("boxes/u_a/ckpt", &src, 0).await.unwrap_err()
        );
        assert_eq!(fake.called("PUT"), before + 1);
        assert!(e.contains("403"), "{e}");
        assert!(
            !e.contains(KEY_ID) && !e.contains(SECRET) && !e.contains("AWS4"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
