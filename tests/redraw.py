"""Real PTYs: publishing updates every client without repainting stable body text."""
import contextlib
import fcntl
import os
from pathlib import Path
import pty
import select
import shlex
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get('TMUX_STATUS_BIN', ROOT/'target/release/tmux-status')).resolve()
MARKER = b'STABLE_SELECTION_SENTENCE'


def main():
    with tempfile.TemporaryDirectory(prefix='status-redraw-') as tmp:
        base = Path(tmp)
        sockets = []
        clients = []
        def tmux(sock, *args):
            return subprocess.check_output(['tmux', '-S', str(sock), *args], text=True,
                                           stderr=subprocess.STDOUT, timeout=8).strip()
        def attach(sock):
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 140, 0, 0))
            process = subprocess.Popen(['tmux', '-S', str(sock), 'attach'], stdin=slave,
                                       stdout=slave, stderr=slave,
                                       env={**os.environ, 'TERM':'xterm-256color', 'TMUX':''})
            os.close(slave)
            clients.append((process, master))
            return master
        def drain(fds, seconds):
            data = {fd:b'' for fd in fds}
            until = time.monotonic() + seconds
            while time.monotonic() < until:
                for fd in select.select(fds, [], [], .05)[0]:
                    data[fd] += os.read(fd, 65536)
            return data
        try:
            producer = base/'producer.py'
            producer.write_text("import time\nprint('\\033[?1049h\\033[HSTABLE_SELECTION_SENTENCE',end='',flush=True)\nfor n in range(300):\n print('\\033[15;1Hlive-output-'+str(n),end='',flush=True)\n time.sleep(.1)\n")
            inner = base/'inner'; sockets.append(inner)
            config = base/'tmux.conf'
            config.write_text("set -g default-terminal tmux-256color\nset -g status-interval 30\nset -g status-right-length 120\nset -g @tmux-status-network off\nset -g @tmux-status-interval 1\nset -g @tmux-status-cpu-label 'CPU:'\nset -g @tmux-status-mem-label 'MEM:'\nset -g @tmux-status-bin " + shlex.quote(str(BINARY)) + "\nset -g status-right '#{tmux_status}'\nrun-shell " + shlex.quote(str(ROOT/'tmux-status.tmux')) + '\n')
            tmux(inner, '-f', str(config), 'new', '-d', '-x', '140', '-y', '24',
                 shlex.join(['python3', str(producer)]))
            first = attach(inner)
            second = attach(inner)
            outer = base/'outer'; sockets.append(outer)
            tmux(outer, '-f', '/dev/null', 'new', '-d', '-x', '140', '-y', '24',
                 'env -u TMUX ' + shlex.join(['tmux', '-S', str(inner), 'attach', '-f', 'ignore-size']))
            nested = attach(outer)
            fds = [first, second, nested]
            # Headless PTYs do not answer terminal queries; let tmux's
            # initial capability negotiation timeout finish before measuring.
            initial = drain(fds, 6)
            assert all(MARKER in data for data in initial.values())
            updates = drain(fds, 3.2)
            for fd, data in updates.items():
                assert MARKER not in data, ('stable body repainted', fd)
                assert b'\x1b[2J' not in data, ('screen cleared', fd)
                assert b'CPU:' in data and b'MEM:' in data, ('client not refreshed', fd)
                assert b'live-output-' in data, ('application output frozen', fd)
            value = tmux(inner, 'display', '-p', '#{TMUX_STATUS_LOCAL}')
            assert '%' in value and '#[' in value
            # Hidden state must not leak into child processes.
            output = base/'child-env'
            tmux(inner, 'new-window', '-d', 'env > '+shlex.quote(str(output)))
            for _ in range(40):
                if output.exists(): break
                time.sleep(.025)
            assert 'TMUX_STATUS_LOCAL=' not in output.read_text()
            # Positive control: the former publication API repaints the marker.
            drain(fds, .3)
            tmux(inner, 'set', '-g', '@redraw-positive-control', '1')
            control = drain(fds, .4)
            assert MARKER in control[first], 'test failed to detect old failure mechanism'
            print('PASS: two clients + nested client update; stable body retained; output live; hidden data; positive control')
        finally:
            for sock in reversed(sockets):
                with contextlib.suppress(subprocess.CalledProcessError): tmux(sock, 'kill-server')
            for proc, fd in clients:
                with contextlib.suppress(subprocess.TimeoutExpired): proc.wait(timeout=3)
                if proc.poll() is None: proc.kill(); proc.wait()
                os.close(fd)

if __name__ == '__main__': main()
