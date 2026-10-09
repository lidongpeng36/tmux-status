//! One owner per tmux server. Results are shared native options, not format jobs.
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
    process::{Command, Stdio},
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

fn runtime() -> io::Result<PathBuf> {
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

    pub fn configuration(&self) -> Option<Args> {
        let mut buffer = [0u8; 65536];
        let mut latest = None;
        while let Ok(size) = self.control.recv(&mut buffer) {
            if let Ok(args) = serde_json::from_slice::<Args>(&buffer[..size])
                && args.serve.as_ref() == Some(&self.socket)
                && args.server_pid == Some(self.server_pid)
                && args.appearance.validate().is_ok()
            {
                latest = Some(args);
            }
        }
        latest
    }

    pub fn publish(&self, snapshot: &Snapshot, args: &Args) -> io::Result<bool> {
        let local = render::profile(snapshot, args, false);
        let remote = render::profile(snapshot, args, true);
        let json = serde_json::to_string(snapshot)?;
        // One child per published snapshot, no shell parsing and no attach client.
        // This connection never keeps a session/server alive.
        let status = Command::new(&self.tmux)
            .arg("-S")
            .arg(&self.socket)
            .args([
                "set-option",
                "-gq",
                "@tmux-status-local",
                &local,
                ";",
                "set-option",
                "-gq",
                "@tmux-status-remote",
                &remote,
                ";",
                "set-option",
                "-gq",
                "@tmux-status-json",
                &json,
                ";",
                "set-option",
                "-gq",
                "@tmux-status-collector-pid",
                &std::process::id().to_string(),
                ";",
                "set-option",
                "-gq",
                "@tmux-status-collector-version",
                env!("CARGO_PKG_VERSION"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(status.success())
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        // File drop releases flock. No PID-file signalling, no kill-by-name.
    }
}
