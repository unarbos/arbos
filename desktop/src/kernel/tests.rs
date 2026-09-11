use super::*;
use std::{
    io::{Read, Write},
    sync::{
        Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct Endpoint {
    info: WebInfo,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Endpoint {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let info = WebInfo {
            url: format!("http://{}", listener.local_addr().unwrap()),
            pid: 0,
            started: 0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let _ = stream.read(&mut [0; 4096]);
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            info,
            stop,
            worker: Some(worker),
        }
    }

    fn tunnel(&self) -> Tunnel {
        Tunnel {
            child: Command::new("sleep").arg("60").spawn().unwrap(),
            info: self.info.clone(),
            http: None,
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn concurrent_remote_attaches_share_one_live_tunnel() {
    let endpoint = Endpoint::new();
    let key = format!("concurrent-{}", endpoint.info.url);
    let started = Barrier::new(8);
    let creations = AtomicUsize::new(0);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    started.wait();
                    attach_remote_cached(&key, || {
                        creations.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(100));
                        Ok(endpoint.tunnel())
                    })
                    .unwrap()
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap().url, endpoint.info.url);
        }
    });
    let mut tunnel = tunnels_lock().remove(&key).unwrap();
    assert_eq!(creations.load(Ordering::SeqCst), 1);
    assert!(tunnel.child.try_wait().unwrap().is_none());
    assert!(alive(&tunnel.info));
}

#[test]
fn failed_remote_attach_can_retry_without_blocking_other_places() {
    let endpoint = Endpoint::new();
    let key = format!("retry-{}", endpoint.info.url);
    assert!(attach_remote_cached(&key, || Err(anyhow!("failed startup"))).is_err());
    let info = attach_remote_cached(&key, || {
        let other = format!("other-{key}");
        let other_info = attach_remote_cached(&other, || Ok(endpoint.tunnel()))?;
        tunnels_lock().remove(&other);
        assert_eq!(other_info.url, endpoint.info.url);
        Ok(endpoint.tunnel())
    })
    .unwrap();
    tunnels_lock().remove(&key);
    assert_eq!(info.url, endpoint.info.url);
}

#[test]
#[ignore = "requires ARBOS_TEST_SSH_HOST and ARBOS_TEST_SSH_PATH"]
fn concurrent_remote_attach_live_websockets() {
    let host = std::env::var("ARBOS_TEST_SSH_HOST").unwrap();
    let path = PathBuf::from(std::env::var("ARBOS_TEST_SSH_PATH").unwrap());
    let started = Barrier::new(8);
    let infos = thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    started.wait();
                    attach_remote(&host, &path).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    let key = Place::remote(&host, &path).encode();
    let tunnel = tunnels_lock().remove(&key).unwrap();
    for info in &infos {
        assert_eq!(info.url, tunnel.info.url);
    }
    crate::agent::acp::runtime().block_on(async {
        let mut sockets = Vec::new();
        for info in &infos {
            let (socket, _) = tokio::time::timeout(
                Duration::from_secs(10),
                tokio_tungstenite::connect_async(websocket_url(info)),
            )
            .await
            .unwrap()
            .unwrap();
            sockets.push(socket);
        }
        for mut socket in sockets {
            socket.close(None).await.unwrap();
        }
    });
    assert!(alive(&tunnel.info));
    eprintln!(
        "{} concurrent attaches and websocket handshakes passed for {key}",
        infos.len()
    );
}
