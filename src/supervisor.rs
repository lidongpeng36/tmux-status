//! One tmux-owned monitor per server; only it owns and signals its worker group.
use crate::{Args, process, publisher};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static CHILD_EVENT: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();
static WATCH_CHILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn child_changed() -> bool {
    let pending = CHILD_EVENT
        .get()
        .is_some_and(|flag| flag.swap(false, std::sync::atomic::Ordering::Relaxed));
    pending && WATCH_CHILD.load(std::sync::atomic::Ordering::Relaxed)
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        // The process group was created by this monitor; never signal a PID read
        // from tmux metadata. Also clean subprocesses after the leader exits.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGTERM);
            libc::kill(-(self.0.id() as i32), libc::SIGCONT);
        }
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until {
            if self.0.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}
fn log(file: &mut File, text: &str) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = writeln!(file, "{stamp} {text}");
}
fn rpc(args: &Args, words: &[&str]) -> io::Result<std::process::Output> {
    process::output(
        Command::new(&args.tmux_bin)
            .arg("-S")
            .arg(args.serve.as_ref().unwrap())
            .args(words),
    )
}
fn state(args: &Args, text: &str) {
    let _ = rpc(
        args,
        &[
            "set-environment",
            "-gh",
            "TMUX_STATUS_STATE",
            text,
            ";",
            "set-environment",
            "-gh",
            "TMUX_STATUS_LOCAL",
            "#[fg=colour214]STATUS:STALE#[default]",
            ";",
            "set-environment",
            "-gh",
            "TMUX_STATUS_REMOTE",
            "#[fg=colour214]STATUS:STALE#[default]",
            ";",
            "set-environment",
            "-gh",
            "TMUX_STATUS_JSON",
            r#"{"cpu_percent":null,"memory":null,"load_per_core":null,"battery":null,"battery_error":false,"reachability":"unknown"}"#,
        ],
    );
    let _ = publisher::refresh(&args.tmux_bin, args.serve.as_ref().unwrap());
}
fn same_server(args: &Args) -> io::Result<bool> {
    let out = process::check(
        rpc(args, &["display-message", "-p", "#{pid}"])?,
        "server identity",
    )?;
    Ok(String::from_utf8_lossy(&out.stdout).trim() == args.server_pid.unwrap().to_string())
}

pub fn run(args: Args) -> io::Result<()> {
    let child_event = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    signal_hook::flag::register(libc::SIGCHLD, child_event.clone())?;
    let _ = CHILD_EVENT.set(child_event);
    let root = publisher::runtime()?;
    let pid = args.server_pid.unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join(format!("{pid}.monitor.lock")))?;
    let meta = lock.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::getuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err(io::Error::other("unsafe monitor lock"));
    }
    match lock.try_lock_exclusive() {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // Reload updates the existing worker's config. During a restart gap
            // the launcher will read current tmux options before spawning it.
            let _ = publisher::Publisher::enter(&args)?;
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    let path = root.join(format!("{pid}.log"));
    if fs::symlink_metadata(&path)
        .is_ok_and(|m| !m.is_file() || m.nlink() != 1 || m.uid() != unsafe { libc::getuid() })
    {
        return Err(io::Error::other("unsafe monitor log"));
    }
    let mut logfile = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let heartbeat = root.join(format!("{pid}.heartbeat"));
    let mut delay = 1u64;
    log(&mut logfile, "monitor starting");
    let mut first = true;
    loop {
        if crate::stopped() {
            break;
        }
        // A transient RPC error is not proof of server death. The original
        // stdout pipe and PID lifetime determine shutdown; verify identity
        // before ever starting another child against a socket.
        match same_server(&args) {
            Ok(true) => (),
            Ok(false) => {
                log(&mut logfile, "server identity changed; monitor stopped");
                break;
            }
            Err(error) => {
                if unsafe { libc::kill(pid as i32, 0) } == -1
                    && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                log(
                    &mut logfile,
                    &format!("server identity unavailable: {error}; retry in {delay}s"),
                );
                if !crate::wait_for_next_sample(Instant::now() + Duration::from_secs(delay))? {
                    break;
                }
                delay = (delay * 2).min(30);
                continue;
            }
        }
        if logfile.metadata()?.len() > 262144 {
            logfile.set_len(0)?;
            log(&mut logfile, "log truncated at 256 KiB limit");
        }
        let _ = rpc(
            &args,
            &[
                "set-environment",
                "-gh",
                "TMUX_STATUS_SUPERVISOR_PID",
                &std::process::id().to_string(),
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_SUPERVISOR_VERSION",
                env!("CARGO_PKG_VERSION"),
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_LOG",
                &path.to_string_lossy(),
            ],
        );
        state(&args, if first { "starting" } else { "restarting" });
        first = false;
        let spawned = Command::new(args.supervise.as_ref().unwrap())
            .args([
                args.serve.as_ref().unwrap().as_os_str(),
                std::ffi::OsStr::new(&pid.to_string()),
            ])
            .env("TMUX_STATUS_WORKER", "1")
            .env("TMUX_BIN", &args.tmux_bin)
            .stdout(Stdio::inherit())
            .stderr(logfile.try_clone()?)
            .process_group(0)
            .spawn();
        match spawned {
            Err(e) => log(&mut logfile, &format!("worker launch failed: {e}")),
            Ok(child) => {
                let mut child = Worker(child);
                log(
                    &mut logfile,
                    &format!("worker started pid={}", child.0.id()),
                );
                WATCH_CHILD.store(true, std::sync::atomic::Ordering::Relaxed);
                let started = Instant::now();
                let mut progress = started;
                let mut last = String::new();
                let mut ttl = args.interval.saturating_mul(3).max(15);
                let mut check_interval = Duration::from_secs(5);
                loop {
                    if logfile.metadata()?.len() > 262144 {
                        logfile.set_len(0)?;
                        log(&mut logfile, "log truncated at 256 KiB limit");
                    }
                    if let Some(status) = child.0.try_wait()? {
                        log(
                            &mut logfile,
                            &format!("worker exited pid={} status={status}", child.0.id()),
                        );
                        break;
                    }
                    if let Ok(text) = fs::read_to_string(&heartbeat) {
                        let fields: Vec<_> = text.split_whitespace().collect();
                        if fields.len() >= 3
                            && fields[0] == child.0.id().to_string()
                            && text != last
                        {
                            ttl = fields[1]
                                .parse::<u64>()
                                .unwrap_or(args.interval)
                                .clamp(1, 3600)
                                .saturating_mul(3)
                                .max(15);
                            check_interval = if fields.get(3) == Some(&"paused") {
                                Duration::from_secs(30)
                            } else {
                                Duration::from_secs(
                                    fields[1].parse::<u64>().unwrap_or(5).clamp(1, 30),
                                )
                            };
                            progress = Instant::now();
                            last = text;
                        }
                    }
                    if progress.elapsed() >= Duration::from_secs(ttl) {
                        log(
                            &mut logfile,
                            "worker heartbeat expired; terminating owned process group",
                        );
                        break;
                    }
                    if !crate::wait_for_next_sample(Instant::now() + check_interval)? {
                        log(
                            &mut logfile,
                            "tmux pipe closed or monitor termination requested",
                        );
                        return Ok(());
                    }
                }
                WATCH_CHILD.store(false, std::sync::atomic::Ordering::Relaxed);
                drop(child);
                if started.elapsed() >= Duration::from_secs(30) {
                    delay = 1;
                }
            }
        }
        state(&args, "restarting");
        log(&mut logfile, &format!("retry in {delay}s"));
        if !crate::wait_for_next_sample(Instant::now() + Duration::from_secs(delay))? {
            break;
        }
        delay = (delay * 2).min(30);
    }
    log(&mut logfile, "monitor stopped");
    Ok(())
}
