//! A place that serves the feed but is behind must not hide a release another place already
//! names. The site mirrors the GitHub release asset on a timer, so between a publish and that
//! refresh the mirror answers 200 with a well-formed but older appcast. Taking that as the
//! answer told every stable Mac it was up to date (0.6.378 against 0.6.379/0.6.380,
//! 2026-11-03), because the fetch stopped at the first feed that parsed.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use arbos_update::client::fetch_appcast_from;
use arbos_update::fetch::Retry;

/// A feed naming `versions`, newest first.
fn feed(versions: &[&str]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rss version=\"2.0\" xmlns:sparkle=\"http://www.andymatuschak.org/xml-namespaces/sparkle\">\n <channel><title>Arbos (stable)</title>\n",
    );
    for v in versions {
        let build = v.rsplit('.').next().unwrap_or(v);
        xml.push_str(&format!(
            "  <item><title>{v}</title><sparkle:version>{build}</sparkle:version>\
             <sparkle:shortVersionString>{v}</sparkle:shortVersionString>\
             <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>\
             <enclosure url=\"https://github.com/unarbos/arbos/releases/download/v{v}/Arbos-{v}-macos-universal.tar.gz\" \
             length=\"100\" type=\"application/octet-stream\" sparkle:os=\"macos\" sparkle:edSignature=\"sig-{v}\"/></item>\n"
        ));
    }
    xml.push_str(" </channel>\n</rss>\n");
    xml
}

/// Serve `body` to every request, or close the connection when `body` is `None`.
async fn serve(body: Option<String>) -> (SocketAddr, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits: Arc<Mutex<usize>> = Arc::default();
    let counted = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            *counted.lock().unwrap() += 1;
            let body = body.clone();
            tokio::spawn(async move {
                let Some(body) = body else {
                    drop(sock);
                    return;
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => buf.extend_from_slice(&tmp[..k]),
                    }
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (addr, hits)
}

fn quick() -> Retry {
    Retry {
        attempts: 2,
        base: Duration::from_millis(10),
        cap: Duration::from_millis(40),
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_millis(500),
    }
}

fn url(addr: SocketAddr) -> String {
    format!("http://{addr}/appcast.xml")
}

/// The mirror is listed first and is two releases behind. The newest build is still offered,
/// because the GitHub feed is asked as well instead of only when the mirror fails.
#[tokio::test]
async fn a_stale_mirror_cannot_hide_a_newer_release() {
    let (stale, stale_hits) = serve(Some(feed(&["0.6.378", "0.6.377"]))).await;
    let (fresh, fresh_hits) = serve(Some(feed(&["0.6.380", "0.6.379", "0.6.378"]))).await;

    let merged = fetch_appcast_from(&[url(stale), url(fresh)], &quick())
        .await
        .expect("both feeds answer");

    let offered = merged
        .newest_for("0.6.378", "macos", None)
        .expect("0.6.378 is offered something");
    assert_eq!(offered.version, "0.6.380");
    assert_eq!(
        merged
            .items
            .iter()
            .map(|i| i.version.as_str())
            .collect::<Vec<_>>(),
        vec!["0.6.380", "0.6.379", "0.6.378", "0.6.377"],
        "the union, newest first, with 0.6.378 named once"
    );
    assert_eq!(
        (*stale_hits.lock().unwrap(), *fresh_hits.lock().unwrap()),
        (1, 1)
    );
}

/// The fallback that already existed must survive: one place down, the other still answers.
#[tokio::test]
async fn one_feed_down_is_not_a_failed_check() {
    let (down, _) = serve(None).await;
    let (up, _) = serve(Some(feed(&["0.6.380"]))).await;

    let from_second = fetch_appcast_from(&[url(down), url(up)], &quick())
        .await
        .expect("the second feed answers");
    assert_eq!(from_second.items.len(), 1);
    assert_eq!(from_second.items[0].version, "0.6.380");

    let from_first = fetch_appcast_from(&[url(up), url(down)], &quick())
        .await
        .expect("the first feed answers");
    assert_eq!(from_first.items[0].version, "0.6.380");
}

/// Every place down is still a failed check, and the error names both of them — the app must
/// never read that as "up to date".
#[tokio::test]
async fn all_feeds_down_is_an_error_naming_each() {
    let (a, _) = serve(None).await;
    let (b, _) = serve(None).await;
    let err = fetch_appcast_from(&[url(a), url(b)], &quick())
        .await
        .expect_err("nothing answered");
    let text = format!("{err:#}");
    assert!(text.contains(&a.to_string()), "{text}");
    assert!(text.contains(&b.to_string()), "{text}");
}
