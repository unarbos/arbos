//! Local HTTP/account/DTO collaborators around unchanged extracted production bodies.
include!("actual.rs");
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
struct Shared {
    http: reqwest::Client,
    http_url: String,
    session_owner: u64,
    active: AtomicBool,
    token: Mutex<String>,
    refreshes: AtomicUsize,
    switch_on_refresh: AtomicBool,
    errors: Mutex<Vec<UserError>>,
}
impl Shared {
    fn new(url: String) -> Arc<Self> {
        Arc::new(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            http_url: url,
            session_owner: 1,
            active: AtomicBool::new(true),
            token: Mutex::new("synthetic-A".into()),
            refreshes: AtomicUsize::new(0),
            switch_on_refresh: AtomicBool::new(false),
            errors: Mutex::new(vec![]),
        })
    }
    fn blob_slot(&self, id: &str) -> String {
        id.to_owned()
    }
    fn token(&self) -> String {
        self.token.lock().unwrap().clone()
    }
    fn token_for_owner(&self, owner: u64) -> Result<String> {
        if owner != self.session_owner || !self.active.load(Ordering::SeqCst) {
            return Err(anyhow!("retired account"));
        }
        Ok(self.token())
    }
    async fn refresh_tokens(&self, _: Option<&str>, owner: Option<u64>) -> Result<()> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if self.switch_on_refresh.load(Ordering::SeqCst) {
            self.active.store(false, Ordering::SeqCst);
            return Err(anyhow!("retired account"));
        }
        let t = self.token_for_owner(owner.unwrap())?;
        assert_eq!(t, "synthetic-A");
        *self.token.lock().unwrap() = "synthetic-A2".into();
        Ok(())
    }
    fn fail<'a>(&self, verb: &str, fault: impl Into<Fault<'a>>) {
        self.fail_with(fault.into().classify(verb))
    }
    fn fail_with(&self, error: UserError) {
        self.errors.lock().unwrap().push(error)
    }
}
#[derive(Clone, Debug, Serialize)]
struct Request {
    method: String,
    path: String,
    auth: String,
    length: Option<usize>,
    chunked: bool,
    mime: String,
    bytes: usize,
    hash: u64,
    small: Vec<u8>,
}
#[derive(Default)]
struct ServerData {
    records: Mutex<Vec<Request>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    get_active: AtomicUsize,
    get_peak: AtomicUsize,
    delay_ms: AtomicUsize,
    large_bytes: AtomicUsize,
}
struct Server {
    url: String,
    data: Arc<ServerData>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort()
    }
}
fn peak(p: &AtomicUsize, n: usize) {
    p.fetch_max(n, Ordering::SeqCst);
}
impl Server {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let data = Arc::new(ServerData::default());
        let d = data.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let d = d.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap().to_owned();
                    let path = parts.next().unwrap().to_owned();
                    let mut headers = std::collections::HashMap::new();
                    loop {
                        line.clear();
                        reader.read_line(&mut line).await.unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        let (k, v) = line.split_once(':').unwrap();
                        headers.insert(k.to_ascii_lowercase(), v.trim().to_owned());
                    }
                    let n = d.active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak(&d.peak, n);
                    if method == "GET" {
                        let n = d.get_active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak(&d.get_peak, n);
                    }
                    let delay = if path.contains("slow-fail") {
                        45
                    } else if path.contains("fast-fail") {
                        1
                    } else {
                        d.delay_ms.load(Ordering::SeqCst)
                    };
                    tokio::time::sleep(Duration::from_millis(delay as u64)).await;
                    let length = headers
                        .get("content-length")
                        .map(|s| s.parse::<usize>().unwrap());
                    let chunked = headers
                        .get("transfer-encoding")
                        .is_some_and(|s| s == "chunked");
                    assert!(!chunked, "unexpected chunked request");
                    let mut left = length.unwrap_or(0);
                    let mut bytes = 0;
                    let mut hash = 14695981039346656037u64;
                    let mut small = vec![];
                    let mut buf = [0u8; 65536];
                    while left > 0 {
                        let cap = left.min(buf.len());
                        let n = reader.read(&mut buf[..cap]).await.unwrap();
                        if n == 0 {
                            break;
                        }
                        for b in &buf[..n] {
                            hash = (hash ^ (*b as u64)).wrapping_mul(1099511628211)
                        }
                        if bytes + n <= 1024 * 1024 {
                            small.extend_from_slice(&buf[..n])
                        }
                        bytes += n;
                        left -= n;
                    }
                    let auth = headers.get("authorization").cloned().unwrap_or_default();
                    let status = if method == "PUT"
                        && path.contains("redirect")
                        && !path.contains("redirected")
                    {
                        307
                    } else if method == "GET"
                        && (path.contains("expired") || path.contains("switch"))
                        && auth == "Bearer synthetic-A"
                    {
                        401
                    } else if path.contains("unauth") {
                        401
                    } else if path.contains("fast-fail") {
                        503
                    } else if path.contains("fail") {
                        500
                    } else {
                        200
                    };
                    d.records.lock().unwrap().push(Request {
                        method: method.clone(),
                        path: path.clone(),
                        auth,
                        length,
                        chunked,
                        mime: headers.get("content-type").cloned().unwrap_or_default(),
                        bytes,
                        hash,
                        small,
                    });
                    let mut stream = reader.into_inner();
                    let error = b"{\"error\":{\"type\":\"token_expired\",\"message\":\"expired\"}}";
                    let count = if status != 200 {
                        error.len()
                    } else if method == "GET" {
                        if path.contains("large") {
                            d.large_bytes.load(Ordering::SeqCst)
                        } else {
                            8192
                        }
                    } else {
                        0
                    };
                    let location = if status == 307 {
                        format!("Location: {}\r\n", path.replace("redirect", "redirected"))
                    } else {
                        String::new()
                    };
                    let header=format!("HTTP/1.1 {status} fixture\r\n{location}Content-Type: image/png\r\nContent-Length: {count}\r\nConnection: close\r\n\r\n");
                    if stream.write_all(header.as_bytes()).await.is_ok() {
                        if status != 200 {
                            let _ = stream.write_all(error).await;
                        } else if method == "GET" {
                            let body = [b'G'; 65536];
                            let mut remaining = count;
                            while remaining > 0 {
                                let n = remaining.min(body.len());
                                if stream.write_all(&body[..n]).await.is_err() {
                                    break;
                                }
                                remaining -= n;
                            }
                        }
                    }
                    let _ = stream.shutdown().await;
                    d.active.fetch_sub(1, Ordering::SeqCst);
                    if method == "GET" {
                        d.get_active.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        Self { url, data, task }
    }
    fn records(&self) -> Vec<Request> {
        self.data.records.lock().unwrap().clone()
    }
}
fn source() -> arbos_core::doc::AttachmentSource {
    arbos_core::doc::AttachmentSource {
        provider: "fixture".into(),
        id: "item".into(),
        page_url: "https://example.invalid/item".into(),
    }
}
fn att(name: &str, key: Option<&str>) -> Attachment {
    Attachment {
        name: name.into(),
        mime: "image/png".into(),
        blob_ref: key.map(str::to_owned),
        bytes: None,
        kind: Some(AttachmentKind::Image),
        title: None,
        chart_part: None,
        url: None,
        source: None,
    }
}
fn copy(name: &str, key: &str) -> Outgoing {
    Outgoing::Copy {
        from_slot: "origin".into(),
        attachment: att(name, Some(key)),
    }
}
fn names(items: &[UserAttachment]) -> Vec<String> {
    items.iter().map(|x| x.name.clone()).collect()
}
fn saved(items: &[UserAttachment]) -> Vec<bool> {
    items.iter().map(|x| x.blob_ref.is_some()).collect()
}
fn error_order(shared: &Shared) -> Vec<String> {
    shared
        .errors
        .lock()
        .unwrap()
        .iter()
        .map(|e| format!("{} {}", e.verb, e.raw))
        .collect()
}
fn hash(data: &[u8]) -> u64 {
    data.iter().fold(14695981039346656037u64, |h, b| {
        (h ^ (*b as u64)).wrapping_mul(1099511628211)
    })
}
async fn flow(shared: Arc<Shared>, out: Vec<Outgoing>) -> Vec<UserAttachment> {
    upload_flow(shared, "target".into(), out, None, None)
        .await
        .0
}
pub async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("controls");
    let root = PathBuf::from(std::env::var("FIXTURE_TMP")?);
    tokio::fs::create_dir_all(&root).await?;
    if mode == "bench" || mode == "memory"
        || mode == "allocation-check"  || mode == "bounds-check" {
        let count = args[2].parse::<usize>()?;
        let bytes = args[3].parse::<usize>()?;
        let server = Server::new().await;
        server.data.delay_ms.store(12, Ordering::SeqCst);
        server.data.large_bytes.store(bytes, Ordering::SeqCst);
        let s = Shared::new(server.url.clone());
        let mut out = vec![];
        let kind = args.get(4).map(String::as_str).unwrap_or("path");
        let mut retained = vec![];
        for i in 0..count {
            if kind == "sticker" || kind == "shared-sticker" || (kind == "mixed" && i % 2 == 0) {
                let bytes = Arc::new(vec![i as u8; bytes]);
                if kind == "shared-sticker" {
                    retained.push(bytes.clone());
                }
                out.push(Outgoing::Sticker {
                    name: format!("image-{i}.png"),
                    mime: "image/png".into(),
                    bytes,
                    source: source(),
                });
            } else if kind == "copy" {
                out.push(copy(&format!("file-{i}.png"), "large"))
            } else {
                let path = root.join(format!("file-{i}.png"));
                tokio::fs::write(&path, vec![i as u8; bytes]).await?;
                out.push(Outgoing::Path(path));
            }
        }
        let baseline = crate::heap_current();
        crate::heap_target(bytes);
        let allocated_before = crate::heap_total();
        crate::heap_reset();
        let t = Instant::now();
        let items = flow(s.clone(), out).await;
        let ms = t.elapsed().as_secs_f64() * 1000.;
        let total_peak = crate::heap_peak();
        let allocated = crate::heap_total() - allocated_before;
        let target_allocs = crate::heap_target_count();
        let memory = total_peak.saturating_sub(baseline);
        assert_eq!(items.len(), count);
        assert!(saved(&items).iter().all(|x| *x));
        assert!(s.errors.lock().unwrap().is_empty());
        let r = server.records();
        assert_eq!(r.iter().filter(|r| r.method == "PUT").count(), count);
        for (i, r) in r.iter().filter(|r| r.method == "PUT").enumerate() {
            assert_eq!(r.bytes, bytes);
            assert_eq!(r.length, Some(bytes));
            if kind == "shared-sticker" || kind == "sticker" {
                let expected = (0..bytes).fold(14695981039346656037u64, |h, _| (h ^ i as u64).wrapping_mul(1099511628211));
                assert_eq!(r.hash, expected);
            }
            if kind == "copy" {
                let expected = (0..bytes).fold(14695981039346656037u64, |h, _| (h ^ u64::from(b'G')).wrapping_mul(1099511628211));
                assert_eq!(r.hash, expected);
            }
        }
        if mode == "allocation-check" {
            assert_eq!(kind, "copy");
            assert!(bytes >= 4 * 1024 * 1024);
            assert_eq!(count, 1);
            assert_eq!(target_allocs, 1, "forwarding should not allocate a second full payload Vec");
        }
        if mode == "bounds-check" {
            assert_eq!(
                server.data.peak.load(Ordering::SeqCst),
                1,
                "only one newly buffered payload pipeline may upload at once"
            );
        }
        println!(
            "{}",
            json!({"mode":mode,"kind":kind,"count":count,"bytes_each":bytes,"milliseconds":ms,"peak_http":server.data.peak.load(Ordering::SeqCst),"peak_get":server.data.get_peak.load(Ordering::SeqCst),"heap_baseline_bytes":baseline,"heap_peak_delta_bytes":memory,"heap_peak_bytes":total_peak,"allocation_requested_bytes":allocated,"payload_sized_allocation_events":target_allocs,"put_receipts":r.iter().filter(|r|r.method=="PUT").map(|r|json!({"bytes":r.bytes,"length":r.length,"hash":r.hash,"mime":r.mime})).collect::<Vec<_>>(),"names":names(&items),"saved":saved(&items)})
        );
        return Ok(());
    }
    if mode == "redirect" {
        let kind = args.get(2).map(String::as_str).unwrap_or("path");
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let outgoing = match kind {
            "copy" => copy("redirect.png", "image"),
            "sticker" => Outgoing::Sticker {
                name: "redirect.png".into(),
                mime: "image/png".into(),
                bytes: Arc::new(b"owned file bytes".to_vec()),
                source: source(),
            },
            _ => {
                let p = root.join("redirect.png");
                tokio::fs::write(&p, b"owned file bytes").await?;
                Outgoing::Path(p)
            }
        };
        let items = flow(s.clone(), vec![outgoing]).await;
        let r = server.records();
        let puts = r.iter().filter(|r| r.method == "PUT").collect::<Vec<_>>();
        assert_eq!(saved(&items), vec![true]);
        assert_eq!(puts.len(), 2);
        assert_eq!(puts[0].hash, puts[1].hash);
        assert_eq!(puts[0].length, puts[1].length);
        println!(
            "{}",
            json!({"kind":kind,"saved":saved(&items),"requests":r,"errors":error_order(&s)})
        );
        return Ok(());
    }

    let mut cases = vec![];
    // Actual byte/length/MIME, picker metadata, and zero-network GIF behavior.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let p = root.join("hello Ω.PNG");
        let b = (0..4096).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        tokio::fs::write(&p, &b).await?;
        let items = flow(
            s.clone(),
            vec![
                Outgoing::Path(p),
                Outgoing::Sticker {
                    name: "sticker.webp".into(),
                    mime: "image/webp".into(),
                    bytes: Arc::new(vec![9; 37]),
                    source: source(),
                },
                Outgoing::Gif {
                    name: "gif.gif".into(),
                    mime: "image/gif".into(),
                    url: "https://example.invalid/a.gif".into(),
                    source: source(),
                },
            ],
        )
        .await;
        assert_eq!(
            names(&items),
            vec!["hello Ω.PNG", "sticker.webp", "gif.gif"]
        );
        assert_eq!(saved(&items), vec![true, true, false]);
        assert_eq!(
            items[2].url.as_deref(),
            Some("https://example.invalid/a.gif")
        );
        assert_eq!(items[1].kind.as_deref(), Some("sticker"));
        assert_eq!(items[1].source.as_ref().unwrap().id, "item");
        let r = server.records();
        assert_eq!(r.len(), 2);
        let file = r.iter().find(|r| r.path.contains("hello_")).unwrap();
        assert_eq!(file.bytes, b.len());
        assert_eq!(file.length, Some(b.len()));
        assert_eq!(file.mime, "image/png");
        assert_eq!(file.hash, hash(&b));
        assert_eq!(file.auth, "Bearer synthetic-A");
        assert_eq!(r.iter().find(|r|r.path.contains("sticker.webp")).unwrap().small,vec![9;37]);
        cases.push("path-length-bytes-mime-and-picker-metadata");
    }
    // Opening once preserves the original inode even if its pathname is replaced.
    {
        let server = Server::new().await;
        server.data.delay_ms.store(80, Ordering::SeqCst);
        let s = Shared::new(server.url.clone());
        let p = root.join("owned.png");
        let old = root.join("owned-renamed.png");
        let bytes = vec![13; 8 * 1024 * 1024];
        tokio::fs::write(&p, &bytes).await?;
        let input = p.clone();
        let job = tokio::spawn(async move { flow(s, vec![Outgoing::Path(input)]).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while server.data.active.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
        tokio::fs::rename(&p, &old).await?;
        tokio::fs::write(&p, vec![99; 17]).await?;
        let items = job.await?;
        assert_eq!(saved(&items), vec![true]);
        let r = server.records();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].bytes, bytes.len());
        assert_eq!(r[0].length, Some(bytes.len()));
        assert_eq!(r[0].hash, hash(&bytes));
        cases.push("path-replacement-retains-open-original-file-and-length");
    }
    // Missing input has a visible note and ref=None; it does not abort later uploads.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let items = flow(
            s.clone(),
            vec![
                Outgoing::Path(root.join("missing.png")),
                Outgoing::Copy {
                    from_slot: "source".into(),
                    attachment: att("no-ref", None),
                },
                copy("ok.png", "ok"),
            ],
        )
        .await;
        assert_eq!(saved(&items), vec![false, false, true]);
        let e = error_order(&s);
        assert_eq!(e.len(), 2);
        assert!(e[0].starts_with("attach missing.png"));
        assert!(e[1].starts_with("attach no-ref"));
        cases.push("read-failure-order-and-ref-none-continues");
    }
    // PUT401 was not retried on the original source and must remain one attempt.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let p = root.join("unauth.txt");
        tokio::fs::write(&p, b"must-not-replay").await?;
        let items = flow(s.clone(), vec![Outgoing::Path(p)]).await;
        assert_eq!(saved(&items), vec![false]);
        assert_eq!(s.refreshes.load(Ordering::SeqCst), 0);
        assert_eq!(server.records().len(), 1);
        assert_eq!(error_order(&s).len(), 1);
        cases.push("put-401-one-attempt-no-refresh");
    }
    // Other downloads still use the original mime/Vec helper contract.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let (mime, bytes) = fetch_blob_bytes(&s, "source", "expired").await?;
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, vec![b'G'; 8192]);
        assert_eq!(s.refreshes.load(Ordering::SeqCst), 1);
        cases.push("download-helper-mime-bytes-refresh-preserved");
    }
    // Copy refresh reads the exact original owner's replacement access token once.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let items = flow(s.clone(), vec![copy("copied.png", "expired")]).await;
        assert_eq!(saved(&items), vec![true]);
        let r = server.records();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].auth, "Bearer synthetic-A");
        assert_eq!(r[1].auth, "Bearer synthetic-A2");
        assert_eq!(r[2].auth, "Bearer synthetic-A2");
        assert_eq!(r[2].hash, hash(&vec![b'G'; 8192]));
        assert_eq!(s.refreshes.load(Ordering::SeqCst), 1);
        cases.push("copy-owner-refresh-bytes-and-length");
    }
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        s.switch_on_refresh.store(true, Ordering::SeqCst);
        let items = flow(s.clone(), vec![copy("retired.png", "switch")]).await;
        assert_eq!(saved(&items), vec![false]);
        assert_eq!(server.records().len(), 1);
        assert_eq!(s.refreshes.load(Ordering::SeqCst), 1);
        cases.push("copy-refuses-retired-owner-before-retry-or-put");
    }
    // Request completion may differ, but user failure notes must retain input order.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let mut out = vec![];
        for n in ["slow-fail.txt", "fast-fail.txt"] {
            let p = root.join(n);
            tokio::fs::write(&p, b"fail").await?;
            out.push(Outgoing::Path(p));
        }
        let items = flow(s.clone(), out).await;
        assert_eq!(saved(&items), vec![false, false]);
        let e = error_order(&s);
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|e| e.starts_with("upload the attachment")));
        assert!(e[0].contains("500"));
        assert!(e[1].contains("503"));
        cases.push("failed-puts-visible-in-input-order");
    }
    // Actual voice + fallback tranche retains ordinary, voice, chart metadata order.
    {
        let server = Server::new().await;
        let s = Shared::new(server.url.clone());
        let forwarded = Forwarded {
            from: "author".into(),
            from_slot: "origin".into(),
            charts: vec![ChartBody {
                chart: Part::Chart {
                    id: "old-chart".into(),
                    spec_version: 1,
                    chart: json!({"kind":"bar"}),
                    table: "a,b".into(),
                    fallback: Some("old-image".into()),
                },
                fallback: Some(att("fallback.png", Some("image"))),
            }],
        };
        let (items, parts) = upload_flow(
            s,
            "target".into(),
            vec![Outgoing::Gif {
                name: "first.gif".into(),
                mime: "image/gif".into(),
                url: "https://example.invalid/a.gif".into(),
                source: source(),
            }],
            Some(Arc::new(vec![7; 120])),
            Some(forwarded),
        )
        .await;
        assert_eq!(names(&items), vec!["first.gif", "voice-memo.wav"]);
        assert_eq!(items[1].kind.as_deref(), Some("audio"));
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["attachment"]["chartPart"], parts[1]["id"]);
        assert_eq!(parts[1]["fallback"], parts[0]["id"]);
        assert!(parts[0]["attachment"]["ref"].is_string());
        let r = server.records();
        assert!(r[0].path.contains("voice-"));
        assert_eq!(r[0].small, vec![7; 120]);
        assert_eq!(r[0].length, Some(120));
        assert_eq!(r[0].mime, "audio/wav");
        cases.push("voice-and-forwarded-chart-linked-display-order");
    }
    // Actual queue keeps a later text behind all uploads in the same chat.
    {
        let server = Server::new().await;
        server.data.delay_ms.store(20, Ordering::SeqCst);
        let s = Shared::new(server.url.clone());
        let done = Arc::new(Mutex::new(vec![]));
        let q = super::chat_send_queue::ChatSendQueue::new(|_, _| panic!("fixture timeout"));
        let p = root.join("fifo.png");
        tokio::fs::write(&p, b"fifo").await?;
        let a = done.clone();
        q.push("chat".into(), "image".into(), async move {
            flow(s, vec![Outgoing::Path(p)]).await;
            a.lock().unwrap().push("image")
        });
        let a = done.clone();
        q.push("chat".into(), "text".into(), async move {
            a.lock().unwrap().push("text")
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if done.lock().unwrap().len() == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
        assert_eq!(*done.lock().unwrap(), vec!["image", "text"]);
        q.clear();
        cases.push("actual-chat-send-queue-fifo");
    }
    // Clearing the actual queue aborts the in-flight upload and discards later work.
    {
        let server = Server::new().await;
        server.data.delay_ms.store(100, Ordering::SeqCst);
        let s = Shared::new(server.url.clone());
        let done = Arc::new(Mutex::new(vec![]));
        let q = super::chat_send_queue::ChatSendQueue::new(|_, _| panic!("fixture timeout"));
        let p = root.join("cancel.png");
        tokio::fs::write(&p, b"cancel").await?;
        let a = done.clone();
        q.push("chat".into(), "image".into(), async move {
            flow(s, vec![Outgoing::Path(p)]).await;
            a.lock().unwrap().push("image")
        });
        let a = done.clone();
        q.push("chat".into(), "text".into(), async move {
            a.lock().unwrap().push("text")
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while server.data.active.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
        q.clear();
        tokio::time::sleep(Duration::from_millis(130)).await;
        assert!(done.lock().unwrap().is_empty());
        cases.push("actual-send-queue-clear-aborts-and-drops-later-send");
    }
    println!(
        "{}",
        json!({"cases":cases,"count":cases.len(),"scope":"actual selected production upload/fetch/tranche/classifier/ChatSendQueue; synthetic local collaborators and loopback HTTP"})
    );
    Ok(())
}
