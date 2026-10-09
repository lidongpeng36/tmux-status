mod metrics;
mod network;
mod platform;
mod render;

use clap::Parser;
use metrics::{Reachability, Snapshot};
use std::{
    io::{self, Write},
    net::SocketAddr,
    time::{Duration, Instant},
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Native system metrics. Streams one line per interval; tmux displays the latest line."
)]
pub struct Args {
    /// Print a single snapshot and exit (CPU is unknown without a previous sample).
    #[arg(long)]
    once: bool,
    /// Output JSON Lines instead of tmux markup.
    #[arg(long)]
    json: bool,
    /// Plain text with ASCII separators and battery label.
    #[arg(long, conflicts_with = "json")]
    plain: bool,
    /// CPU/memory/output interval, seconds.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=3600))]
    interval: u64,
    /// Battery query interval, seconds. Also caches "no battery" and errors.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    battery_interval: u64,
    #[arg(long)]
    no_battery: bool,
    #[arg(long)]
    no_date: bool,
    /// Include 1/5/15 minute load averages divided by logical CPUs.
    #[arg(long)]
    load: bool,
    /// Opt-in TCP reachability target, numeric IP:port (IPv6: [address]:port).
    /// No DNS, HTTP request, TLS handshake, or default network traffic.
    #[arg(long)]
    probe: Option<SocketAddr>,
    /// Successful probe interval, seconds. Failures back off up to 10x.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
    probe_interval: u64,
    /// Hard TCP connection timeout, milliseconds. Probes run off the render thread.
    #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(1..=5000))]
    probe_timeout_ms: u64,
}

#[cfg(target_os = "linux")]
fn wait_for_next_sample(deadline: Instant) -> io::Result<bool> {
    // Poll only for pipe closure/errors, not POLLOUT (which is always ready).
    // This lets tmux own the lifecycle even when our next write is minutes away.
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(true);
        }
        let timeout = deadline
            .duration_since(now)
            .as_millis()
            .min(i32::MAX as u128) as i32;
        let mut fd = libc::pollfd {
            fd: libc::STDOUT_FILENO,
            events: 0,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, writable for the duration of the call.
        let n = unsafe { libc::poll(&mut fd, 1, timeout.max(1)) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Ok(false);
        }
    }
}

#[cfg(target_os = "macos")]
fn wait_for_next_sample(deadline: Instant) -> io::Result<bool> {
    use nix::sys::{
        event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue},
        time::TimeSpec,
    };
    // Darwin poll(events=0) ignores a pipe's lost reader. EV_CLEAR consumes
    // initial writability once, then blocks until EOF or the sample deadline.
    let queue = Kqueue::new().map_err(io::Error::from)?;
    let event = KEvent::new(
        libc::STDOUT_FILENO as usize,
        EventFilter::EVFILT_WRITE,
        EvFlags::EV_ADD | EvFlags::EV_CLEAR,
        FilterFlag::empty(),
        0,
        0,
    );
    let mut events = [event];
    queue
        .kevent(
            &[event],
            &mut [],
            Some(*TimeSpec::from_duration(Duration::ZERO).as_ref()),
        )
        .map_err(io::Error::from)?;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(true);
        }
        let timeout = TimeSpec::from_duration(deadline.duration_since(now));
        let count = match queue.kevent(&[], &mut events, Some(*timeout.as_ref())) {
            Ok(count) => count,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(io::Error::from(e)),
        };
        if count > 0
            && events[0]
                .flags()
                .intersects(EvFlags::EV_EOF | EvFlags::EV_ERROR)
        {
            return Ok(false);
        }
    }
}

fn run(args: Args) -> io::Result<()> {
    let timeout = Duration::from_millis(args.probe_timeout_ms);
    let receiver = if args.once {
        None
    } else {
        args.probe.map(|target| {
            network::worker(target, timeout, Duration::from_secs(args.probe_interval))
        })
    };
    let mut network_update: Option<network::Update> = None;
    let mut collector = platform::Collector::new();
    let mut battery = None;
    let mut battery_error = false;
    let mut next_battery = Instant::now();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        let started = Instant::now();
        let (cpu_percent, memory, load_per_core) = collector.sample(args.load);
        if !args.no_battery && started >= next_battery {
            match platform::battery() {
                Ok(value) => {
                    battery = value;
                    battery_error = false;
                }
                Err(_) => {
                    battery = None;
                    battery_error = true;
                }
            }
            next_battery = started + Duration::from_secs(args.battery_interval);
        }
        if let Some(rx) = &receiver {
            for update in rx.try_iter() {
                network_update = Some(update);
            }
        }
        let reachability = args.probe.map(|target| {
            if args.once {
                network::probe(target, timeout)
            } else if let Some(update) = &network_update {
                if started < update.expires {
                    update.state
                } else {
                    Reachability::Unknown
                }
            } else {
                Reachability::Unknown
            }
        });
        let snapshot = Snapshot {
            cpu_percent,
            memory,
            load_per_core,
            battery,
            battery_error,
            reachability,
        };
        let line = if args.json {
            serde_json::to_string(&snapshot)?
        } else {
            render::render(&snapshot, &args)
        };
        writeln!(output, "{line}")?;
        output.flush()?;
        if args.once || !wait_for_next_sample(started + Duration::from_secs(args.interval))? {
            return Ok(());
        }
    }
}

fn main() {
    if let Err(e) = run(Args::parse())
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("tmux-status: {e}");
        std::process::exit(1);
    }
}
