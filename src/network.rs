use crate::metrics::Reachability;
use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::{
    net::{SocketAddr, TcpStream},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

pub const DEFAULT_URLS: [&str; 2] = [
    "https://cp.cloudflare.com/generate_204",
    "https://connectivitycheck.gstatic.com/generate_204",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub tcp: Option<SocketAddr>,
    pub http: bool,
    pub urls: Vec<String>,
    pub direct: bool,
    pub timeout_ms: u64,
    pub interval: u64,
}

pub fn valid_url(text: &str) -> Result<String, String> {
    let url = Url::parse(text).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("check URL must be HTTP(S), with a host and without credentials".into());
    }
    Ok(url.into())
}

pub fn probe(target: SocketAddr, timeout: Duration) -> Reachability {
    match TcpStream::connect_timeout(&target, timeout) {
        Ok(_) => Reachability::Reachable,
        Err(_) => Reachability::Unreachable,
    }
}

fn client(config: &Config) -> reqwest::Result<Client> {
    let timeout = Duration::from_millis(config.timeout_ms);
    let mut builder = Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(Policy::none())
        .user_agent(concat!("tmux-status/", env!("CARGO_PKG_VERSION")))
        .pool_max_idle_per_host(0); // Fresh connections measure current connectivity, not an old keepalive.
    if config.direct {
        builder = builder.no_proxy();
    }
    builder.build()
}

fn http_probe(client: &Client, urls: &[String]) -> Reachability {
    let mut unexpected_response = false;
    for url in urls {
        match client.get(url).header("Cache-Control", "no-cache").send() {
            Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT => {
                return Reachability::Reachable;
            }
            Ok(_) => unexpected_response = true,
            Err(_) => (),
        }
    }
    if unexpected_response {
        Reachability::Limited
    } else {
        Reachability::Unreachable
    }
}

pub fn once(config: &Config) -> Reachability {
    if let Some(target) = config.tcp {
        return probe(target, Duration::from_millis(config.timeout_ms));
    }
    match client(config) {
        Ok(client) => http_probe(&client, &config.urls),
        Err(_) => Reachability::Unknown,
    }
}

fn next_delay(base: Duration, failures: u32) -> Duration {
    base.saturating_mul(1u32 << failures.min(4))
        .min(base.saturating_mul(10))
}

// Native interface/address snapshots let reconnects reset backoff without
// continuous external requests. This is not a claim to detect every route change.
fn fingerprint(networks: &mut sysinfo::Networks) -> Vec<String> {
    networks.refresh(true);
    let mut interfaces: Vec<_> = networks
        .iter()
        .map(|(name, data)| {
            let mut ips: Vec<_> = data
                .ip_networks()
                .iter()
                .map(|ip| format!("{ip:?}"))
                .collect();
            ips.sort();
            format!("{name}:{ips:?}")
        })
        .collect();
    interfaces.sort();
    interfaces
}

pub struct Update {
    pub state: Reachability,
    pub expires: Instant,
    pub config: Config,
}

pub struct Worker {
    pub updates: Receiver<Update>,
    config: Sender<Config>,
}

impl Worker {
    pub fn configure(&self, config: Config) {
        let _ = self.config.send(config);
    }
}

pub fn worker(initial: Config) -> Worker {
    let (tx, updates) = mpsc::channel();
    let (config_tx, config_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut config = initial;
        let mut networks = sysinfo::Networks::new();
        let mut addresses = fingerprint(&mut networks);
        let mut failures: u32 = 0;
        let mut next = Instant::now();
        let mut http_client = if config.http {
            client(&config).ok()
        } else {
            None
        };
        loop {
            if (config.http || config.tcp.is_some()) && Instant::now() >= next {
                let state = if let Some(target) = config.tcp {
                    probe(target, Duration::from_millis(config.timeout_ms))
                } else if let Some(client) = &http_client {
                    http_probe(client, &config.urls)
                } else {
                    Reachability::Unknown
                };
                failures = if state == Reachability::Reachable {
                    0
                } else {
                    failures.saturating_add(1u32)
                };
                let delay = next_delay(Duration::from_secs(config.interval), failures);
                next = Instant::now() + delay;
                let expires = next
                    + Duration::from_millis(config.timeout_ms * config.urls.len().max(1) as u64)
                    + Duration::from_secs(2);
                if tx
                    .send(Update {
                        state,
                        expires,
                        config: config.clone(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            let wait = if config.http || config.tcp.is_some() {
                Duration::from_secs(5)
                    .min(next.saturating_duration_since(Instant::now()))
                    .max(Duration::from_millis(50))
            } else {
                Duration::from_secs(5)
            };
            match config_rx.recv_timeout(wait) {
                Ok(mut latest) => {
                    for queued in config_rx.try_iter() {
                        latest = queued;
                    }
                    if latest != config {
                        config = latest;
                        http_client = if config.http {
                            client(&config).ok()
                        } else {
                            None
                        };
                        failures = 0;
                        next = Instant::now();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
            if config.http || config.tcp.is_some() {
                let current = fingerprint(&mut networks);
                if current != addresses {
                    addresses = current;
                    failures = 0;
                    next = Instant::now();
                }
            }
        }
    });
    Worker {
        updates,
        config: config_tx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    fn endpoint(response: &'static str) -> (String, thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 1024];
            let size = stream.read(&mut buffer).unwrap();
            assert!(size > 0);
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{addr}/"), handle)
    }
    fn config() -> Config {
        Config {
            tcp: None,
            http: true,
            urls: vec![],
            direct: true,
            timeout_ms: 100,
            interval: 30,
        }
    }
    #[test]
    fn http_requires_204_and_falls_back_without_following_redirects() {
        let client = client(&config()).unwrap();
        let (bad, bad_handle) = endpoint(
            "HTTP/1.1 302 Found\r\nLocation: http://example.invalid/\r\nContent-Length: 0\r\n\r\n",
        );
        let (good, good_handle) = endpoint("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        assert_eq!(http_probe(&client, &[bad, good]), Reachability::Reachable);
        bad_handle.join().unwrap();
        good_handle.join().unwrap();
        let (portal, handle) = endpoint("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nlogin");
        assert_eq!(http_probe(&client, &[portal]), Reachability::Limited);
        handle.join().unwrap();
    }
    #[test]
    fn tcp_success_and_refusal_and_backoff() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        assert_eq!(
            probe(addr, Duration::from_millis(100)),
            Reachability::Reachable
        );
        drop(listener);
        assert_eq!(
            probe(addr, Duration::from_millis(100)),
            Reachability::Unreachable
        );
        assert_eq!(
            next_delay(Duration::from_secs(30), 0),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_delay(Duration::from_secs(30), 1),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_delay(Duration::from_secs(30), 99),
            Duration::from_secs(300)
        );
        assert!(valid_url("file:///tmp/x").is_err());
        assert!(valid_url("https://user:password@example.com/").is_err());
    }
}
