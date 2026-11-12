//! The daemon's second door (docs/box-scaling.md §4.1 and Phase 4): for a relay on another host.
//!
//! TLS 1.3, and the caller proves itself too: its certificate must chain to the operator's CA
//! (`ARBOS_BOXD_TLS_CLIENT_CA_FILE`) or the handshake ends and no request is read. The bearer
//! token is asked for all the same, by the router behind this. Off unless
//! `ARBOS_BOXD_TLS_LISTEN` is set.

use std::net::SocketAddr;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

/// A caller gets this long to finish its handshake, and this many may be in one at a time: a
/// peer that has not proved itself holds a socket and nothing else.
const HANDSHAKE: Duration = Duration::from_secs(10);
const MAX_HANDSHAKES: usize = 64;

pub struct Cfg {
    listen: SocketAddr,
    server: Arc<ServerConfig>,
}

/// `ARBOS_BOXD_TLS_LISTEN` and the three files it needs. Unset: no second door. Set with any
/// file missing or loose: the daemon does not start.
pub fn from_env() -> Result<Option<Cfg>> {
    let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let Some(listen) = var("ARBOS_BOXD_TLS_LISTEN") else {
        return Ok(None);
    };
    let file = |key: &str| {
        var(key).with_context(|| format!("ARBOS_BOXD_TLS_LISTEN is set; {key} must name a file"))
    };
    Ok(Some(Cfg {
        listen: listen
            .trim()
            .parse()
            .context("ARBOS_BOXD_TLS_LISTEN must be <ip>:<port>")?,
        server: server_config(
            &file("ARBOS_BOXD_TLS_CERT_FILE")?,
            &file("ARBOS_BOXD_TLS_KEY_FILE")?,
            &file("ARBOS_BOXD_TLS_CLIENT_CA_FILE")?,
        )?,
    }))
}

/// A plain file, not a link, that nobody but its owner can write, and, when it is the key, that
/// nobody but its owner can read.
fn checked<'a>(what: &str, path: &'a str, secret: bool) -> Result<&'a str> {
    let md = std::fs::symlink_metadata(path).with_context(|| format!("{what} {path}"))?;
    let (loose, mode) = if secret {
        (0o077, "with mode 0600")
    } else {
        (0o022, "only its owner can write")
    };
    if !md.is_file() || md.mode() & loose != 0 {
        bail!("{what} {path} must be a plain file {mode}");
    }
    Ok(path)
}

/// The daemon's certificate and key, and the one CA a caller's certificate must chain to. What
/// a file fails to parse as is not repeated: one of them is a key.
pub fn server_config(cert: &str, key: &str, client_ca: &str) -> Result<Arc<ServerConfig>> {
    let certs = |what: &str, path: &str| -> Result<Vec<CertificateDer<'static>>> {
        CertificateDer::pem_file_iter(checked(what, path, false)?)
            .and_then(Iterator::collect)
            .ok()
            .filter(|certs: &Vec<_>| !certs.is_empty())
            .ok_or_else(|| anyhow!("{what} {path} holds no PEM certificate"))
    };
    let mut roots = RootCertStore::empty();
    for ca in certs("client CA file", client_ca)? {
        roots
            .add(ca)
            .map_err(|_| anyhow!("client CA file {client_ca} holds a certificate that is no CA"))?;
    }
    let chain = certs("certificate file", cert)?;
    let key = PrivateKeyDer::from_pem_file(checked("key file", key, true)?)
        .map_err(|_| anyhow!("key file {key} holds no PEM private key"))?;
    // Named, not the process default: the workspace builds rustls with more than one provider.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    // No `allow_unauthenticated`: a caller with no certificate is refused like one with a wrong one.
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .context("client CA")?;
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3")?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .map_err(|_| anyhow!("{cert} is not the certificate of the key in its key file"))?;
    Ok(Arc::new(config))
}

/// Connections that finished the handshake, for `axum::serve`.
pub struct Listener {
    pub local: SocketAddr,
    shaken: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
}

impl Cfg {
    pub async fn bind(self) -> Result<Listener> {
        let tcp = TcpListener::bind(self.listen)
            .await
            .with_context(|| format!("ARBOS_BOXD_TLS_LISTEN {}", self.listen))?;
        let local = tcp.local_addr()?;
        let acceptor = TlsAcceptor::from(self.server);
        let (tx, shaken) = mpsc::channel(MAX_HANDSHAKES);
        let slots = Arc::new(Semaphore::new(MAX_HANDSHAKES));
        // Each handshake is its own task, so a caller that stalls in one holds up nobody.
        tokio::spawn(async move {
            loop {
                let Ok(slot) = slots.clone().acquire_owned().await else {
                    return;
                };
                let (stream, peer) = match tcp.accept().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        // Out of descriptors, most likely: wait rather than spin.
                        tracing::warn!(error = %e, "tls: accept failed");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    match tokio::time::timeout(HANDSHAKE, acceptor.accept(stream)).await {
                        Ok(Ok(tls)) => {
                            let _ = tx.send((tls, peer)).await;
                        }
                        // No certificate, one from another CA, or not TLS at all.
                        Ok(Err(e)) => tracing::warn!(%peer, error = %e, "tls: caller refused"),
                        Err(_) => tracing::warn!(%peer, "tls: handshake timed out"),
                    }
                });
            }
        });
        Ok(Listener { local, shaken })
    }
}

impl axum::serve::Listener for Listener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.shaken.recv().await {
            Some(conn) => conn,
            // The accept task is gone, and with it the door: nothing more will arrive.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::boxes::tests::boxd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// A CA of two days in `dir` as `<ca>.pem`, and for each name a key and a certificate it
    /// signed for 127.0.0.1, as `<name>.key` and `<name>.pem`. Made by openssl(1), which every
    /// host these tests run on has.
    pub fn pki(dir: &Path, ca: &str, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        let at = |file: String| dir.join(file).to_str().unwrap().to_owned();
        let openssl = |args: &[&str]| {
            let out = std::process::Command::new("openssl")
                .args(args)
                .output()
                .expect("openssl on PATH");
            assert!(out.status.success(), "openssl {args:?}: {out:?}");
        };
        let key = |name: &str| {
            let path = at(format!("{name}.key"));
            openssl(&[
                "ecparam",
                "-name",
                "prime256v1",
                "-genkey",
                "-noout",
                "-out",
                &path,
            ]);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            path
        };
        let (ca_key, ca_pem) = (key(ca), at(format!("{ca}.pem")));
        let subject = format!("/CN={ca}");
        openssl(&[
            "req", "-x509", "-new", "-key", &ca_key, "-out", &ca_pem, "-subj", &subject, "-days",
            "2",
        ]);
        let ext = at(format!("{ca}.ext"));
        std::fs::write(&ext, "subjectAltName=IP:127.0.0.1\n").unwrap();
        for name in names {
            let (key, csr, pem) = (
                key(name),
                at(format!("{name}.csr")),
                at(format!("{name}.pem")),
            );
            let subject = format!("/CN={name}");
            openssl(&["req", "-new", "-key", &key, "-out", &csr, "-subj", &subject]);
            openssl(&[
                "x509",
                "-req",
                "-in",
                &csr,
                "-CA",
                &ca_pem,
                "-CAkey",
                &ca_key,
                "-CAcreateserial",
                "-out",
                &pem,
                "-days",
                "2",
                "-extfile",
                &ext,
            ]);
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arbos-boxd-tls-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A client that trusts `ca` alone and shows `name`'s certificate, if any.
    fn client(dir: &Path, ca: &str, name: Option<&str>) -> reqwest::Client {
        let read = |file: String| std::fs::read(dir.join(file)).unwrap();
        let ca = reqwest::Certificate::from_pem(&read(format!("{ca}.pem"))).unwrap();
        let mut http = reqwest::Client::builder()
            .tls_built_in_root_certs(false)
            .add_root_certificate(ca);
        if let Some(name) = name {
            let mut pem = read(format!("{name}.pem"));
            pem.extend(read(format!("{name}.key")));
            http = http.identity(reqwest::Identity::from_pem(&pem).unwrap());
        }
        http.build().unwrap()
    }

    /// The door opens to a caller whose certificate the operator's CA signed, and to no other:
    /// none, one from another CA, or no TLS at all never reaches a route. Through it, the
    /// bearer is still asked for.
    #[tokio::test]
    async fn only_a_certificate_from_the_ca_gets_a_request_through() {
        let dir = scratch("door");
        pki(&dir, "ca", &["node", "relay"]);
        pki(&dir, "other", &["stranger"]);
        let file = |name: &str| dir.join(name).to_str().unwrap().to_owned();
        let server = server_config(&file("node.pem"), &file("node.key"), &file("ca.pem")).unwrap();
        let cfg = Cfg {
            listen: "127.0.0.1:0".parse().unwrap(),
            server,
        };
        let listener = cfg.bind().await.unwrap();
        let port = listener.local.port();
        let (boxd, _fake, root) = boxd("tls").await;
        let app = crate::http::router(boxd, TOKEN);
        tokio::spawn(async move { axum::serve(listener, app).await });
        let url = format!("https://127.0.0.1:{port}/v1/host/status");

        let relay = client(&dir, "ca", Some("relay"));
        let ok = relay.get(&url).bearer_auth(TOKEN).send().await.unwrap();
        assert_eq!(ok.status(), 200);
        assert_eq!(relay.get(&url).send().await.unwrap().status(), 401);

        let no_certificate = client(&dir, "ca", None);
        let another_ca = client(&dir, "ca", Some("stranger"));
        for refused in [no_certificate, another_ca] {
            assert!(refused.get(&url).bearer_auth(TOKEN).send().await.is_err());
        }
        // A caller that does not trust this CA does not send the bearer here either.
        let untrusting = client(&dir, "other", Some("relay"));
        assert!(untrusting
            .get(&url)
            .bearer_auth(TOKEN)
            .send()
            .await
            .is_err());
        let plain = format!("http://127.0.0.1:{port}/v1/host/status");
        assert!(relay.get(&plain).bearer_auth(TOKEN).send().await.is_err());
        // The door still opens after all of those.
        let ok = relay.get(&url).bearer_auth(TOKEN).send().await.unwrap();
        assert_eq!(ok.status(), 200);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The daemon does not open the door on a key others can read, a file others can write, a
    /// link, a certificate that is not the key's, or a CA file with no certificate in it.
    #[test]
    fn the_files_must_be_private_whole_and_a_pair() {
        let dir = scratch("files");
        pki(&dir, "ca", &["node", "relay"]);
        let file = |name: &str| dir.join(name).to_str().unwrap().to_owned();
        let mode = |name: &str, mode: u32| {
            std::fs::set_permissions(file(name), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let open = |cert: &str, key: &str, ca: &str| {
            server_config(&file(cert), &file(key), &file(ca)).map(|_| ())
        };
        open("node.pem", "node.key", "ca.pem").unwrap();
        assert!(
            open("node.pem", "relay.key", "ca.pem").is_err(),
            "not a pair"
        );
        assert!(open("node.pem", "node.key", "node.key").is_err(), "no CA");
        assert!(open("node.pem", "node.key", "missing.pem").is_err());
        mode("node.key", 0o640);
        let loose = open("node.pem", "node.key", "ca.pem").unwrap_err();
        assert!(loose.to_string().contains("0600"), "{loose}");
        mode("node.key", 0o600);
        mode("ca.pem", 0o666);
        assert!(open("node.pem", "node.key", "ca.pem").is_err());
        mode("ca.pem", 0o644);
        std::os::unix::fs::symlink(file("node.key"), file("link.key")).unwrap();
        assert!(open("node.pem", "link.key", "ca.pem").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
