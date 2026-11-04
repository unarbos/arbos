//! A check that reaches nothing must try again before it gives up.
//!
//! On 2026-11-03 a Mac was told about 0.6.380 and then failed ten checks in a row, every one of
//! them `getaddrinfo` answering `nodename nor servname provided, or not known` for both
//! arbos.life and github.com while the Mac was online. macOS answers that way for a few
//! seconds after a wake or a network change, before the system resolver has its configuration.
//! Each check asked once, treated that as the answer, and left the Mac waiting out the rest of
//! the five-minute poll interval.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use arbos_update::client::fetch_appcast_from;
use arbos_update::fetch::Retry;

const FEED: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
    "<rss version=\"2.0\" xmlns:sparkle=\"http://www.andymatuschak.org/xml-namespaces/sparkle\">\n",
    " <channel><title>Arbos (stable)</title>\n",
    "  <item><title>0.6.380</title><sparkle:version>380</sparkle:version>",
    "<sparkle:shortVersionString>0.6.380</sparkle:shortVersionString>",
    "<sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>",
    "<enclosure url=\"https://github.com/unarbos/arbos/releases/download/v0.6.380/Arbos-0.6.380-macos-universal.tar.gz\" ",
    "length=\"100\" type=\"application/octet-stream\" sparkle:os=\"macos\" sparkle:edSignature=\"sig\"/></item>\n",
    " </channel>\n</rss>\n",
);

/// Drop the first `refuse` connections without answering, then serve `body` to the rest.
/// Dropping a connection the server has already accepted is what the app sees for a name that
/// did not resolve: the request ends with nothing from the other side.
async fn flaky(refuse: usize, body: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let nth = counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                if nth < refuse {
                    drop(sock);
                    return;
                }
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

/// Answer `404` to everything: a verdict, not a connection that failed.
async fn gone() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => buf.extend_from_slice(&tmp[..k]),
                    }
                }
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (addr, hits)
}

fn quick(attempts: u32) -> Retry {
    Retry {
        attempts,
        base: Duration::from_millis(10),
        cap: Duration::from_millis(40),
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_millis(500),
    }
}

fn url(addr: SocketAddr) -> String {
    format!("http://{addr}/appcast.xml")
}

/// Two rounds fail, the third answers, and the check reports 0.6.380 instead of an error.
#[tokio::test]
async fn a_momentary_failure_no_longer_costs_the_whole_check() {
    let (addr, hits) = flaky(2, FEED).await;

    let appcast = fetch_appcast_from(&[url(addr)], &quick(5))
        .await
        .expect("the third try answers");

    assert_eq!(
        appcast
            .newest_for("0.6.378", "macos", None)
            .map(|i| i.version.clone()),
        Some("0.6.380".to_string()),
        "0.6.380 is offered to 0.6.378"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 3, "two failed tries, then one");
}

/// One try is still only one try when that is all the schedule allows, so a caller that asks
/// for a single attempt does not silently get five.
#[tokio::test]
async fn one_attempt_asks_once() {
    let (addr, hits) = flaky(1, FEED).await;
    fetch_appcast_from(&[url(addr)], &quick(1))
        .await
        .expect_err("the only try failed");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// A 404 is the server's answer and reads the same next time. Retrying it would only delay the
/// failed check, so the round is not repeated.
#[tokio::test]
async fn a_server_that_answers_is_not_asked_again() {
    let (addr, hits) = gone().await;
    let err = fetch_appcast_from(&[url(addr)], &quick(5))
        .await
        .expect_err("404 is not a feed");
    assert!(format!("{err:#}").contains("404"), "{err:#}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "asked once, not five times");
}

/// A feed that answers is never held up by one that does not: the first round already has an
/// answer, so there is no second round.
#[tokio::test]
async fn a_working_feed_is_not_delayed_by_a_broken_one() {
    let (broken, broken_hits) = flaky(usize::MAX, FEED).await;
    let (working, working_hits) = flaky(0, FEED).await;

    let appcast = fetch_appcast_from(&[url(broken), url(working)], &quick(5))
        .await
        .expect("the working feed answers");

    assert_eq!(appcast.items.len(), 1);
    assert_eq!(broken_hits.load(Ordering::SeqCst), 1);
    assert_eq!(working_hits.load(Ordering::SeqCst), 1);
}
