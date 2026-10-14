mod appearance;
mod metrics;
mod network;
mod platform;
mod policy;
mod process;
mod publisher;
mod render;
mod supervisor;

use clap::Parser;
use metrics::{Reachability, Snapshot};
use std::{
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn stopped() -> bool {
    STOP.get().is_some_and(|flag| flag.load(Ordering::Relaxed))
}

#[derive(Clone, Debug, Parser, serde::Serialize, serde::Deserialize)]
#[command(
    version,
    about = "Native system metrics. Streams one line per interval; tmux displays the latest line."
)]
pub struct Args {
    /// Validate options/appearance without starting a collector or probing network.
    #[arg(long)]
    #[serde(default)]
    check_config: bool,
    /// Disable automatic attach/focus/power throttling in server mode.
    #[arg(long)]
    #[serde(default)]
    no_adaptive: bool,
    /// Wake the existing collector after a tmux client state change.
    #[arg(long, requires = "serve")]
    #[serde(skip)]
    wake: bool,
    /// Supervise the TPM launcher, restarting failed workers with backoff.
    #[arg(long, requires = "serve")]
    #[serde(skip)]
    supervise: Option<PathBuf>,
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
    /// Enable HTTP connectivity checks (automatically enabled by the TPM plugin).
    #[arg(long, conflicts_with = "probe")]
    network: bool,
    /// HTTP(S) connectivity-check URL expected to return 204. Repeat for fallback targets.
    #[arg(long, requires = "network", value_parser = network::valid_url)]
    check_url: Vec<String>,
    /// Bypass environment/macOS system proxies for HTTP checks.
    #[arg(long)]
    network_direct: bool,
    /// Whole HTTP request timeout, milliseconds (includes DNS/connect/TLS/headers).
    #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u64).range(1..=10000))]
    network_timeout_ms: u64,
    /// Publish shared metrics into this tmux server instead of streaming stdout.
    #[arg(long, requires = "server_pid", conflicts_with_all = ["once", "json", "plain"])]
    serve: Option<PathBuf>,
    #[arg(long, requires = "serve", value_parser = clap::value_parser!(u32).range(1..))]
    server_pid: Option<u32>,
    #[arg(long, default_value = "tmux")]
    tmux_bin: PathBuf,
    /// Partial JSON appearance override; missing values retain built-in defaults.
    #[arg(long = "appearance", default_value = "{}")]
    #[serde(skip)]
    appearance_input: String,
    #[arg(long)]
    #[serde(skip)]
    appearance_file: Option<PathBuf>,
    /// Override the CPU label (including desired trailing space or colon).
    #[arg(long)]
    #[serde(skip)]
    cpu_label: Option<String>,
    #[arg(long)]
    #[serde(skip)]
    mem_label: Option<String>,
    #[arg(long)]
    #[serde(skip)]
    separator: Option<String>,
    #[arg(long, value_parser = ["icons", "text"])]
    #[serde(skip)]
    labels: Option<String>,
    #[arg(skip)]
    #[serde(default, serialize_with = "appearance::serialize_overrides")]
    appearance: appearance::Appearance,
    /// Successful probe interval, seconds. Failures back off up to 10x.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
    probe_interval: u64,
    /// Hard TCP connection timeout, milliseconds. Probes run off the render thread.
    #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(1..=5000))]
    probe_timeout_ms: u64,
}

impl Args {
    fn network_config(&self) -> network::Config {
        network::Config {
            paused: false,
            tcp: self.probe,
            http: self.network,
            urls: if self.check_url.is_empty() {
                network::DEFAULT_URLS.iter().map(|s| (*s).into()).collect()
            } else {
                self.check_url.clone()
            },
            direct: self.network_direct,
            timeout_ms: if self.probe.is_some() {
                self.probe_timeout_ms
            } else {
                self.network_timeout_ms
            },
            interval: self.probe_interval,
        }
    }
}

fn wait_for_next_sample(deadline: Instant) -> io::Result<bool> {
    wait_for_event(deadline, None)
}

#[cfg(target_os = "linux")]
fn wait_for_event(deadline: Instant, wake: Option<std::os::fd::RawFd>) -> io::Result<bool> {
    // Poll only for pipe closure/errors, not POLLOUT (which is always ready).
    // This lets tmux own the lifecycle even when our next write is minutes away.
    loop {
        let now = Instant::now();
        if stopped() {
            return Ok(false);
        }
        if now >= deadline || supervisor::child_changed() {
            return Ok(true);
        }
        let timeout = deadline
            .duration_since(now)
            .as_millis()
            .min(i32::MAX as u128) as i32;
        let mut fds = [
            libc::pollfd {
                fd: libc::STDOUT_FILENO,
                events: 0,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout.max(1)) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Ok(false);
        }
        if fds[1].revents & libc::POLLIN != 0 {
            return Ok(true);
        }
    }
}

#[cfg(target_os = "macos")]
fn wait_for_event(deadline: Instant, wake: Option<std::os::fd::RawFd>) -> io::Result<bool> {
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
    let mut changes = vec![event];
    if let Some(fd) = wake {
        changes.push(KEvent::new(
            fd as usize,
            EventFilter::EVFILT_READ,
            EvFlags::EV_ADD | EvFlags::EV_CLEAR,
            FilterFlag::empty(),
            0,
            0,
        ));
    }
    let mut events = [event; 2];
    queue
        .kevent(
            &changes,
            &mut [],
            Some(*TimeSpec::from_duration(Duration::ZERO).as_ref()),
        )
        .map_err(io::Error::from)?;
    loop {
        let now = Instant::now();
        if stopped() {
            return Ok(false);
        }
        if now >= deadline || supervisor::child_changed() {
            return Ok(true);
        }
        let timeout = TimeSpec::from_duration(deadline.duration_since(now));
        let count = match queue.kevent(&[], &mut events, Some(*timeout.as_ref())) {
            Ok(count) => count,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(io::Error::from(e)),
        };
        for event in &events[..count] {
            if event
                .flags()
                .intersects(EvFlags::EV_EOF | EvFlags::EV_ERROR)
            {
                return Ok(false);
            }
            if event.filter() == Ok(EventFilter::EVFILT_READ) {
                return Ok(true);
            }
        }
    }
}

fn run(mut args: Args) -> io::Result<()> {
    let publisher = if args.serve.is_some() {
        match publisher::Publisher::enter(&args)? {
            Some(p) => Some(p),
            None => return Ok(()),
        }
    } else {
        None
    };
    let mut worker: Option<network::Worker> = None;
    let mut configured: Option<network::Config> = None;
    let mut network_update: Option<network::Update> = None;
    let mut collector = platform::Collector::new();
    let mut battery = None;
    let mut battery_error = false;
    let mut next_battery = Instant::now();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut policy = policy::Policy::new();
    let mut power = policy::PowerReader::new();
    let mut previous: Option<policy::Plan> = None;
    let mut next_sample = Instant::now();
    let mut prime = true;
    let mut warm_until = None;
    let mut pause_published = false;
    loop {
        let started = Instant::now();
        let mut wake = false;
        if let Some(owner) = &publisher {
            let (updated, signalled) = owner.configuration();
            wake = signalled;
            if let Some(updated) = updated {
                args = updated;
                wake = true;
            }
        }
        let adaptive = publisher.is_some() && !args.no_adaptive;
        let activity = if adaptive {
            publisher.as_ref().unwrap().activity().unwrap_or_else(|e| {
                eprintln!("tmux-status: activity unavailable: {e}");
                policy::Activity::Active
            })
        } else {
            policy::Activity::Active
        };
        // Do not inspect power sources while paused and still detached.
        let paused = previous.is_some_and(|p| p.paused) && activity == policy::Activity::Detached;
        let plan = if adaptive {
            policy.evaluate(
                started,
                activity,
                if paused {
                    policy::Power::default()
                } else {
                    power.get(wake)
                },
                args.interval,
                args.probe_interval,
            )
        } else {
            policy::Plan::fixed(args.interval, args.probe_interval)
        };
        let mut effective = args.network_config();
        effective.paused = plan.paused;
        effective.interval = plan.network.max(1);
        if !args.once && configured.as_ref() != Some(&effective) {
            if let Some(w) = &worker {
                w.configure(effective.clone());
            } else {
                worker = Some(network::worker(effective.clone()));
            }
            network_update = None;
            configured = Some(effective.clone());
        }
        if plan.paused {
            if let Some(owner) = &publisher {
                if !pause_published {
                    match owner.pause() {
                        Ok(()) => pause_published = true,
                        Err(e) => eprintln!("tmux-status: pause publication failed: {e}"),
                    }
                }
                owner.heartbeat(30, "paused")?;
            }
        } else {
            pause_published = false;
            let resumed = previous.is_some_and(|p| p.paused || plan.sample < p.sample);
            if resumed {
                collector = platform::Collector::new();
                prime = true;
                warm_until = None;
                next_battery = started;
            }
            if previous != Some(plan) || (wake && activity == policy::Activity::Active) {
                next_sample = started;
            }
            if let Some(until) = warm_until {
                next_sample = next_sample.max(until);
            }
            if started >= next_sample {
                let (cpu_percent, memory, load_per_core) =
                    collector.sample(args.load || publisher.is_some());
                if (!args.no_battery || publisher.is_some()) && started >= next_battery {
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
                if let Some(w) = &worker {
                    for update in w.updates.try_iter() {
                        if update.config == effective {
                            network_update = Some(update);
                        }
                    }
                }
                let reachability = (args.probe.is_some() || args.network).then(|| {
                    if args.once {
                        network::once(&effective)
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
                if let Some(owner) = &publisher {
                    if let Err(error) = owner.publish(&snapshot, &args, &plan) {
                        eprintln!("tmux-status: publication failed: {error}");
                        if owner.server_gone() {
                            eprintln!("tmux-status: original server ended; stopping collector");
                            return Ok(());
                        }
                    }
                } else {
                    let line = if args.json {
                        serde_json::to_string(&snapshot)?
                    } else {
                        render::render(&snapshot, &args)
                    };
                    writeln!(output, "{line}")?;
                    output.flush()?;
                }
                warm_until = if prime {
                    Some(started + Duration::from_millis(250))
                } else {
                    None
                };
                next_sample = started
                    + if prime {
                        Duration::from_millis(250)
                    } else {
                        Duration::from_secs(plan.sample)
                    };
                prime = false;
            }
        }
        previous = Some(plan);
        if args.once {
            return Ok(());
        }
        let mut deadline = if plan.paused {
            started + Duration::from_secs(30)
        } else {
            next_sample
        };
        if adaptive {
            deadline = deadline.min(started + Duration::from_secs(30));
            if let Some(transition) = policy.transition_due()
                && transition > started
            {
                deadline = deadline.min(transition);
            }
        }
        if !wait_for_event(deadline, publisher.as_ref().map(|p| p.control_fd()))? {
            return Ok(());
        }
    }
}

fn main() {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        if let Err(error) = signal_hook::flag::register(signal, stop.clone()) {
            eprintln!("tmux-status: signal handler: {error}");
            std::process::exit(1);
        }
    }
    let _ = STOP.set(stop);
    let mut args = Args::parse();
    if args.wake {
        let _ = publisher::wake(args.server_pid.unwrap());
        return;
    }
    let configured = (|| -> Result<appearance::Appearance, String> {
        let mut style =
            appearance::Appearance::load(args.appearance_file.as_deref(), &args.appearance_input)?;
        if let Some(label) = &args.cpu_label {
            style.cpu_label = label.clone();
        }
        if let Some(label) = &args.mem_label {
            style.mem_label = label.clone();
        }
        if let Some(separator) = &args.separator {
            style.separator = separator.clone();
        }
        if let Some(mode) = &args.labels {
            style.labels = if mode == "text" {
                appearance::Labels::Text
            } else {
                appearance::Labels::Icons
            };
        }
        style.validate()?;
        Ok(style)
    })();
    args.appearance = match configured {
        Ok(style) => style,
        Err(error) => {
            eprintln!("tmux-status: appearance: {error}");
            std::process::exit(2);
        }
    };
    if args.check_config {
        return;
    }
    let result = if args.supervise.is_some() {
        supervisor::run(args)
    } else {
        run(args)
    };
    if let Err(e) = result
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("tmux-status: {e}");
        std::process::exit(1);
    }
}
