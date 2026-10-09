"""Exercise a real tmux format job without loading user config or hooks."""
import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time


def main():
    binary = os.path.realpath(os.environ.get("TMUX_STATUS_BIN", "target/release/tmux-status"))
    with tempfile.TemporaryDirectory(prefix="tmux-status-test-") as directory:
        socket = os.path.join(directory, "socket")
        config = os.path.join(directory, "tmux.conf")
        # Our generated absolute test path has no quotes; refuse rather than
        # change the command's meaning if a caller supplies a special path.
        if any(c in binary for c in "'\"$`\\\n"):
            raise ValueError("test binary path contains shell metacharacters")
        with open(config, "w") as stream:
            stream.write("set -g status-interval 1\nset -g status-right-length 130\n"
                         "set -g status-right '#(exec \"" + binary +
                         "\" --interval 1 --plain --no-date)'\n")

        def tmux(*args):
            return subprocess.check_output(["tmux", "-S", socket, *args],
                                           stderr=subprocess.STDOUT, text=True, timeout=5)

        def collectors():
            text = subprocess.check_output(["ps", "-axo", "pid=,ppid=,command="], text=True)
            return [line.strip() for line in text.splitlines()
                    if len(line.split(None, 2)) == 3
                    and line.split(None, 2)[2].startswith(binary + " ")]

        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 160, 0, 0))
        attached = None
        try:
            tmux("-f", config, "new-session", "-d", "-s", "test", "sleep 90")
            attached = subprocess.Popen(["tmux", "-S", socket, "attach-session", "-t", "test"],
                                        stdin=slave, stdout=slave, stderr=slave,
                                        env={**os.environ, "TERM": "xterm-256color"})
            os.close(slave)
            slave = None
            data = b""
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.2)[0]:
                    data += os.read(master, 65536)
            initial = collectors()
            for _ in range(3):
                tmux("source-file", config)
            time.sleep(1)
            after = collectors()
            assert len(initial) == 1 and initial == after, (initial, after)
            assert b"CPU:" in data and b"MEM:" in data, "status not visible"
            tmux("kill-server")
            attached.wait(timeout=3)
            deadline = time.monotonic() + 3
            while collectors() and time.monotonic() < deadline:
                time.sleep(0.1)
            assert not collectors(), "collector survived tmux shutdown"
            print(json.dumps({"status_visible": True, "collector_count": len(initial),
                              "three_reloads_reused_job": True, "shutdown_clean": True}))
        finally:
            try:
                tmux("kill-server")
            except subprocess.CalledProcessError:
                pass
            if attached and attached.poll() is None:
                attached.terminate()
                attached.wait(timeout=3)
            os.close(master)
            if slave is not None:
                os.close(slave)


if __name__ == "__main__":
    main()
