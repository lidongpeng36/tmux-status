//! One owner per tmux server. Results are shared hidden environment values, not format jobs.
use crate::{Args, metrics::Snapshot, render};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixDatagram,
    },
    path::PathBuf,
    process::Command,
    thread,
    time::Duration,
};

pub struct Publisher {
    _lock: File,
    control: UnixDatagram,
    path: PathBuf,
    socket: PathBuf,
    server_pid: u32,
    tmux: PathBuf,
}

fn error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

pub(crate) fn runtime() -> io::Result<PathBuf> {
    // Do not trust a world-writable cache or symlink for IPC/locks.
    let uid = unsafe { libc::getuid() };
    let root = std::env::temp_dir().join(format!("tmux-status-{uid}"));
    match fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let metadata = fs::symlink_metadata(&root)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(error(
            "unsafe runtime directory (must be owned directory, mode 0700)",
        ));
    }
    Ok(root)
}

impl Publisher {
    /// A competing invocation sends its new configuration to the owner and exits.
    pub fn enter(args: &Args) -> io::Result<Option<Self>> {
        let socket = args.serve.as_ref().expect("serve checked by clap");
        let server_pid = args.server_pid.expect("server PID checked by clap");
        let root = runtime()?;
        let path = root.join(format!("{server_pid}.sock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.join(format!("{server_pid}.lock")))?;
        let metadata = lock.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::getuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(error("unsafe lock file"));
        }
        match lock.try_lock_exclusive() {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let client = UnixDatagram::unbound()?;
                let message = serde_json::to_vec(args)?;
                for _ in 0..20 {
                    if client.send_to(&message, &path).is_ok() {
                        return Ok(None);
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "collector owns lock but control socket unavailable",
                ));
            }
            Err(e) => return Err(e),
        }
        // Lock ownership makes any prior IPC socket stale. Never unlink the lock
        // file itself: an open old inode would allow two independent owners.
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let control = UnixDatagram::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        control.set_nonblocking(true)?;
        Ok(Some(Self {
            _lock: lock,
            control,
            path,
            socket: socket.clone(),
            server_pid,
            tmux: args.tmux_bin.clone(),
        }))
    }

    pub fn configuration(&self) -> (Option<Args>, bool) {
        let mut buffer = [0u8; 65536];
        let mut latest = None;
        let mut wake = false;
        while let Ok(size) = self.control.recv(&mut buffer) {
            if &buffer[..size] == b"wake" {
                wake = true;
                continue;
            }
            if let Ok(args) = serde_json::from_slice::<Args>(&buffer[..size])
                && args.serve.as_ref() == Some(&self.socket)
                && args.server_pid == Some(self.server_pid)
                && args.appearance.validate().is_ok()
            {
                latest = Some(args);
            }
        }
        (latest, wake)
    }

    pub fn server_gone(&self) -> bool {
        // An RPC failure alone is not proof that tmux exited.
        let dead = unsafe { libc::kill(self.server_pid as i32, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if dead {
            return true;
        }
        crate::process::output(Command::new(&self.tmux).arg("-S").arg(&self.socket).args([
            "display-message",
            "-p",
            "#{pid}",
        ]))
        .is_ok_and(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout).trim() != self.server_pid.to_string()
        })
    }

    pub fn publish(
        &self,
        snapshot: &Snapshot,
        args: &Args,
        plan: &crate::policy::Plan,
    ) -> io::Result<()> {
        let local = render::profile(snapshot, args, false);
        let remote = render::profile(snapshot, args, true);
        let json = serde_json::to_string(snapshot)?;
        let out =
            crate::process::output(Command::new(&self.tmux).arg("-S").arg(&self.socket).args([
                "set-environment",
                "-gh",
                "TMUX_STATUS_LOCAL",
                &local,
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_REMOTE",
                &remote,
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_JSON",
                &json,
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_COLLECTOR_PID",
                &std::process::id().to_string(),
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_COLLECTOR_VERSION",
                env!("CARGO_PKG_VERSION"),
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_STATE",
                "ok",
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_POLICY",
                plan.label,
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_INTERVAL",
                &plan.sample.to_string(),
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_NETWORK_INTERVAL",
                &plan.network.to_string(),
                ";",
                "set-environment",
                "-gh",
                "TMUX_STATUS_BINARY",
                &std::env::current_exe()?.to_string_lossy(),
            ]))?;
        crate::process::check(out, "publish snapshot")?;
        self.heartbeat(plan.sample, "active")?;
        refresh(&self.tmux, &self.socket)
    }
    pub fn control_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.control.as_raw_fd()
    }
    pub fn activity(&self) -> io::Result<crate::policy::Activity> {
        let out = crate::process::check(
            crate::process::output(Command::new(&self.tmux).arg("-S").arg(&self.socket).args([
                "list-clients",
                "-F",
                "#{client_flags}\t#{client_termfeatures}\t#{focus-events}",
            ]))?,
            "query client activity",
        )?;
        Ok(crate::policy::activity(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }
    pub fn pause(&self) -> io::Result<()> {
        crate::process::check(crate::process::output(Command::new(&self.tmux).arg("-S").arg(&self.socket).args([
            "set-environment","-gh","TMUX_STATUS_STATE","paused",";",
            "set-environment","-gh","TMUX_STATUS_POLICY","paused",";",
            "set-environment","-gh","TMUX_STATUS_INTERVAL","0",";",
            "set-environment","-gh","TMUX_STATUS_NETWORK_INTERVAL","0",";",
            "set-environment","-gh","TMUX_STATUS_LOCAL","STATUS:PAUSED",";",
            "set-environment","-gh","TMUX_STATUS_REMOTE","STATUS:PAUSED",";",
            "set-environment","-gh","TMUX_STATUS_JSON",r#"{"cpu_percent":null,"memory":null,"load_per_core":null,"battery":null,"battery_error":false,"reachability":"unknown"}"#,
        ]))?,"pause metrics")?;
        Ok(())
    }
    pub fn heartbeat(&self, interval: u64, mode: &str) -> io::Result<()> {
        let heartbeat = runtime()?.join(format!("{}.heartbeat", self.server_pid));
        let temporary = heartbeat.with_extension(format!("{}.tmp", std::process::id()));
        use std::io::Write;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        writeln!(
            file,
            "{} {} {} {}",
            std::process::id(),
            interval,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            mode
        )?;
        fs::rename(temporary, heartbeat)?;
        Ok(())
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        // File drop releases flock. No PID-file signalling, no kill-by-name.
    }
}

/// Best effort per-client refresh; disconnected clients are retried next sample.
pub(crate) fn refresh(tmux: &std::path::Path, socket: &std::path::Path) -> io::Result<()> {
    let clients = crate::process::check(
        crate::process::output(Command::new(tmux).arg("-S").arg(socket).args([
            "list-clients",
            "-F",
            "#{client_name}",
        ]))?,
        "list clients",
    )?;
    let names = String::from_utf8_lossy(&clients.stdout);
    let mut command = Command::new(tmux);
    command.arg("-S").arg(socket);
    let mut count = 0;
    for name in names.lines().filter(|s| !s.is_empty()) {
        if count > 0 {
            command.arg(";");
        }
        command.args(["refresh-client", "-S", "-t", name]);
        count += 1;
    }
    if count > 0 {
        let out = crate::process::output(&mut command)?;
        crate::process::check(out, "refresh clients")?;
    }
    Ok(())
}

pub fn wake(pid: u32) -> io::Result<()> {
    let path = runtime()?.join(format!("{pid}.sock"));
    UnixDatagram::unbound()?.send_to(b"wake", path)?;
    Ok(())
}
