use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek},
    os::unix::fs::OpenOptionsExt,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

fn capture() -> io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = crate::publisher::runtime()?.join(format!(
        "rpc-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    std::fs::remove_file(path)?;
    Ok(file)
}
/// Bound tmux RPCs. Unlinked files avoid pipe deadlocks and reader threads when
/// a failed wrapper leaves descendants holding stdout/stderr open.
pub fn output(command: &mut Command) -> io::Result<Output> {
    let mut stdout = capture()?;
    let mut stderr = capture()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux command exceeded 3 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    stdout.rewind()?;
    stderr.rewind()?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.take(65536).read_to_end(&mut out)?;
    stderr.take(65536).read_to_end(&mut err)?;
    Ok(Output {
        status,
        stdout: out,
        stderr: err,
    })
}
pub fn check(output: Output, phase: &str) -> io::Result<Output> {
    if output.status.success() {
        return Ok(output);
    }
    Err(io::Error::other(format!(
        "{phase}: {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}
