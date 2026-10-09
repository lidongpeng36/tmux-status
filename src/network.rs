use crate::metrics::Reachability;
use std::{
    net::{SocketAddr, TcpStream},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub fn probe(target: SocketAddr, timeout: Duration) -> Reachability {
    match TcpStream::connect_timeout(&target, timeout) {
        Ok(_) => Reachability::Reachable,
        Err(_) => Reachability::Unreachable,
    }
}

fn next_delay(base: Duration, failures: u32) -> Duration {
    base.saturating_mul(1u32 << failures.min(4))
        .min(base.saturating_mul(10))
}

pub struct Update {
    pub state: Reachability,
    pub expires: Instant,
}

pub fn worker(target: SocketAddr, timeout: Duration, interval: Duration) -> Receiver<Update> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut failures = 0;
        loop {
            let state = probe(target, timeout);
            failures = if state == Reachability::Reachable {
                0
            } else {
                failures + 1
            };
            let delay = next_delay(interval, failures);
            let update = Update {
                state,
                expires: Instant::now() + delay + timeout + Duration::from_secs(2),
            };
            if tx.send(update).is_err() {
                return;
            }
            thread::sleep(delay);
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
