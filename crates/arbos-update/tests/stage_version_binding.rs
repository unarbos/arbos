//! The downgrade a party serving the appcast could ship, end to end over loopback.
//!
//! `sparkle:edSignature` covers the payload bytes and nothing else, and the only anti-downgrade
//! control is `is_newer` over `sparkle:shortVersionString` — a feed field no signature touches.
//! So an old release's enclosure, a genuine payload with a genuine signature and a published
//! length, can be advertised under a version higher than anything shipped: the check accepts it,
//! the download verifies it, and every Mac on that channel moves to the old build. These tests
//! drive the real `check` → `download` → `stage` path against a stub on 127.0.0.1 and assert
//! that `stage` is where it stops, because by then the version can be compared with the app the
//! signature actually covered.
//!
//! `stage` runs `codesign --verify` only on macOS, so the version check that follows it is
//! reachable here: these tests exercise the real control in its real place, on either OS.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use arbos_update::client::{self, Install};
use arbos_update::sign::PrivateKey;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The `Info.plist` a released bundle carries: the template the Mac app ships, with the two
/// placeholders filled in the way `scripts/bundle-mac.sh` fills them (`__VERSION__` from the
/// workspace version, `__BUILD__` from the build number). Driving the real template means a
/// change to it that the version rule could not read would fail here.
fn released_plist(short: &str, build: &str) -> String {
    std::fs::read_to_string(repo_root().join("crates/arbos-mac/macos/Info.plist"))
        .unwrap()
        .replace("__VERSION__", short)
        .replace("__BUILD__", build)
}

/// An `Arbos.app` whose `Info.plist` is `plist`.
fn write_bundle(root: &Path, plist: &str, label: &str) -> PathBuf {
    let app = root.join("Arbos.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
    std::fs::write(
        app.join("Contents/MacOS/arbos-mac"),
        format!("#!/bin/sh\necho 'arbos-mac {label}'\n"),
    )
    .unwrap();
    app
}

/// A real `.tar.gz` payload with a released-shape `Arbos.app` at its root, what `stage` unpacks.
fn payload_tarball(dir: &Path, short: &str, build: &str) -> Vec<u8> {
    let src = dir.join(format!("src-{short}-{build}"));
    std::fs::create_dir_all(&src).unwrap();
    write_bundle(&src, &released_plist(short, build), "OLD-VULNERABLE");
    let tarball = dir.join(format!("payload-{short}-{build}.tar.gz"));
    let out = std::process::Command::new("tar")
        .args([
            "-czf",
            &tarball.to_string_lossy(),
            "-C",
            &src.to_string_lossy(),
            "Arbos.app",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "tar: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(&tarball).unwrap()
}

/// A feed with one item, pointing at `file` on the stub with that payload's genuine signature
/// and length. The version is whatever the party serving the feed cares to claim.
fn appcast_xml(
    addr: SocketAddr,
    version: &str,
    file: &str,
    payload: &[u8],
    signature: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Arbos (stable)</title>
    <item>
      <title>{version}</title>
      <sparkle:version>{version}</sparkle:version>
      <sparkle:shortVersionString>{version}</sparkle:shortVersionString>
      <enclosure url="http://{addr}/{file}" length="{length}" type="application/octet-stream" sparkle:os="macos" sparkle:edSignature="{signature}"/>
    </item>
  </channel>
</rss>
"#,
        length = payload.len()
    )
}

/// Answers `/appcast.xml` with the feed and anything else with the payload, on 127.0.0.1 only.
/// Nothing leaves the loopback interface: `fetch::feed_urls` and `fetch::download_urls` pass a
/// non-GitHub URL through as is, so no production host is ever consulted.
fn serve(listener: TcpListener, feed: String, payload: Vec<u8>) {
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let feed = feed.clone();
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    head.extend_from_slice(&buf[..n]);
                }
                let body = if String::from_utf8_lossy(&head).contains("/appcast.xml") {
                    feed.into_bytes()
                } else {
                    payload
                };
                let _ = sock
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = sock.write_all(&body).await;
                let _ = sock.flush().await;
                let _ = sock.shutdown().await;
            });
        }
    });
}

/// The whole client path with the feed under someone else's control: poll, download, verify,
/// stage. `file` is the enclosure's name, which is how an old release's asset is named.
async fn offer(
    dir: &Path,
    installed: &str,
    advertised: &str,
    file: &str,
    payload: Vec<u8>,
) -> anyhow::Result<PathBuf> {
    let key = PrivateKey::generate();
    let signature = key.sign(&payload);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    serve(
        listener,
        appcast_xml(addr, advertised, file, &payload, &signature),
        payload,
    );

    let available = client::check(&format!("http://{addr}/appcast.xml"), installed, "macos")
        .await
        .expect("the feed is fetched and parsed")
        .expect("the advertised version is newer than the installed one, so it is offered");
    assert_eq!(available.item.version, advertised);

    let downloaded = client::download(&available, &key.public(), dir, None)
        .await
        .expect("the payload's length and Ed25519 signature are genuine");

    // The installed app whose layout `stage` reads: a released build's bundle name and
    // executable name, so nothing but the version differs.
    let install = Install::MacBundle(write_bundle(
        &dir.join("install"),
        &released_plist(installed, "1"),
        installed,
    ));
    client::stage(&downloaded.path, &available.item.version, dir, &install)
}

/// The attack: an older release's genuine, signed enclosure advertised as 9.9.9.
#[tokio::test]
async fn an_older_signed_payload_advertised_as_newer_is_refused() {
    let dir = tempdir("downgrade");
    let payload = payload_tarball(&dir, "0.1.0", "1");
    let err = offer(
        &dir,
        "0.6.375",
        "9.9.9",
        "Arbos-0.1.0-macos-universal.tar.gz",
        payload,
    )
    .await
    .expect_err("a payload that is not the advertised build must not stage");
    let message = format!("{err:#}");
    assert!(
        message.contains("offered this payload as 9.9.9") && message.contains("0.1.0"),
        "{message}"
    );
    assert!(message.contains("downgrade"), "{message}");
    cleanup(&dir);
}

/// A stable release of the shipped `Info.plist`: the advertised version is the tag, the Cargo
/// version, and `CFBundleShortVersionString`, all the same string.
#[tokio::test]
async fn a_stable_payload_at_its_own_version_stages() {
    let dir = tempdir("stable");
    let payload = payload_tarball(&dir, "0.6.376", "4210");
    let staged = offer(
        &dir,
        "0.6.375",
        "0.6.376",
        "Arbos-0.6.376-macos-universal.tar.gz",
        payload,
    )
    .await
    .expect("the app inside the payload is the advertised build");
    assert!(staged.join("Contents/Info.plist").is_file());
    cleanup(&dir);
}

/// An edge release: the advertised version is `<short>.<build>`, which `bundle-mac.sh` and
/// `update::current_version` both produce, so the rule has to accept it.
#[tokio::test]
async fn an_edge_payload_at_short_plus_build_stages() {
    let dir = tempdir("edge");
    let payload = payload_tarball(&dir, "0.6.376", "4210");
    let staged = offer(
        &dir,
        "0.6.376.4209",
        "0.6.376.4210",
        "Arbos-0.6.376.4210-macos-universal.tar.gz",
        payload,
    )
    .await
    .expect("edge builds are advertised as <CFBundleShortVersionString>.<CFBundleVersion>");
    assert!(staged.join("Contents/Info.plist").is_file());
    cleanup(&dir);
}

/// Replay inside one Cargo version is a downgrade too: an older edge build cannot be
/// re-advertised under a later edge version, because the build numbers differ.
#[tokio::test]
async fn an_edge_payload_at_another_build_is_refused() {
    let dir = tempdir("edge-replay");
    let payload = payload_tarball(&dir, "0.6.376", "4100");
    let err = offer(
        &dir,
        "0.6.376.4209",
        "0.6.376.4210",
        "Arbos-0.6.376.4100-macos-universal.tar.gz",
        payload,
    )
    .await
    .expect_err("build 4100 advertised as build 4210");
    let message = format!("{err:#}");
    assert!(
        message.contains("offered this payload as 0.6.376.4210")
            && message.contains("0.6.376 build 4100"),
        "{message}"
    );
    cleanup(&dir);
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-update-stage-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}
