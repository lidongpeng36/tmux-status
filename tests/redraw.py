"""Real PTYs: publishing updates every client without repainting stable body text."""
import contextlib
import fcntl
import json
import re
import signal
import shutil
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
            producer.write_text("import time\nprint('\\033[?1049h\\033[HSTABLE_SELECTION_SENTENCE',end='',flush=True)\nfor n in range(3000):\n print('\\033[15;1Hlive-output-'+''.join(chr(65+(n+i)%26) for i in range(40)),end='',flush=True)\n time.sleep(.1)\n")
            inner = base/'inner'; sockets.append(inner)
            calls = base/'calls.jsonl'
            injection = base/'injection'
            wrapper = base/'tmux-wrapper'
            real = shutil.which('tmux')
            wrapper.write_text("#!/usr/bin/env python3\n" +
                "import json,os,pathlib,subprocess,sys\n" +
                "args=sys.argv[1:]; real="+repr(real)+"\n" +
                "worker=os.environ.get('TMUX_STATUS_WORKER')=='1'\n" +
                "if worker:\n with open("+repr(str(calls))+",'a') as f: f.write(json.dumps(args)+'\\n')\n" +
                "marker=pathlib.Path("+repr(str(injection))+")\n" +
                "if worker and 'list-clients' in args and marker.exists():\n" +
                " action=marker.read_text(); marker.unlink()\n" +
                " if action=='fail': sys.exit(1)\n" +
                " out=subprocess.run([real]+args,stdout=subprocess.PIPE,stderr=subprocess.PIPE)\n" +
                " subprocess.run([real]+args[:2]+['detach-client','-t',action],check=True)\n" +
                " sys.stdout.buffer.write(out.stdout);sys.stderr.buffer.write(out.stderr);sys.exit(out.returncode)\n" +
                "os.execv(real,[real]+args)\n")
            wrapper.chmod(0o755)
            config = base/'tmux.conf'
            config.write_text("set-environment -g TMUX_BIN " + shlex.quote(str(wrapper)) + "\nset -g default-terminal tmux-256color\nset -g status-interval 30\nset -g status-right-length 120\nset -g @tmux-status-adaptive on\nset -g @tmux-status-network off\nset -g @tmux-status-interval 1\nset -g @tmux-status-cpu-label 'CPU:'\nset -g @tmux-status-mem-label 'MEM:'\nset -g @tmux-status-bin " + shlex.quote(str(BINARY)) + "\nset -g status-right '#{tmux_status}'\nrun-shell " + shlex.quote(str(ROOT/'tmux-status.tmux')) + '\n')
            tmux(inner, '-f', str(config), 'new', '-d', '-x', '140', '-y', '24',
                 shlex.join(['python3', str(producer)]))
            # Python fault injection slows launcher RPCs. Finish initial startup
            # before attaching so concurrent launchers do not obscure the pane.
            deadline = time.monotonic() + 10
            while tmux(inner, 'display', '-p', '#{TMUX_STATUS_STATE}') != 'ok':
                assert time.monotonic() < deadline, 'initial collector startup timed out'
                time.sleep(.05)
            first = attach(inner)
            first_initial = drain([first], 2)[first]
            second = attach(inner)
            early = drain([first, second], 2)
            outer = base/'outer'; sockets.append(outer)
            tmux(outer, '-f', '/dev/null', 'new', '-d', '-x', '140', '-y', '24',
                 'env -u TMUX ' + shlex.join(['tmux', '-S', str(inner), 'attach', '-f', 'ignore-size']))
            nested = attach(outer)
            fds = [first, second, nested]
            # Headless PTYs do not answer terminal queries; let tmux's
            # initial capability negotiation timeout finish before measuring.
            initial = drain(fds, 6)
            assert MARKER in first_initial and MARKER in early[second] and MARKER in initial[nested]
            assert tmux(inner, 'display', '-p', '#{pane_in_mode}') == '0'
            calls.write_text('')
            updates = drain(fds, 4.2)
            commands = [json.loads(line) for line in calls.read_text().splitlines()]
            publications = sum('TMUX_STATUS_COLLECTOR_VERSION' in command for command in commands)
            queries = sum('list-clients' in command for command in commands)
            assert publications >= 3 and abs(queries-publications) <= 1, (queries, publications)
            assert all(command[command.index('-F')+1].startswith('#{client_name}\t#{client_flags}')
                       for command in commands if 'list-clients' in command), commands
            for command in commands:
                assert 'set-option' not in command and 'set' not in command, command
                if 'refresh-client' in command:
                    assert all(command[i+1:i+3] == ['-S', '-t'] for i,x in enumerate(command) if x=='refresh-client'), command
            for fd, data in updates.items():
                assert MARKER not in data, ('stable body repainted', fd)
                assert b'\x1b[2J' not in data, ('screen cleared', fd)
                assert b'CPU:' in data and b'MEM:' in data, ('client not refreshed', fd)
                assert len(set(re.findall(rb'[A-Z]{40}', data))) >= 2, ('application output frozen', fd)
            # Also cover an idle application, matching the original symptom.
            producer_pid = int(tmux(inner, 'display', '-p', '#{pane_pid}'))
            os.kill(producer_pid, signal.SIGSTOP)
            try:
                idle = drain(fds, 3.2)
                assert all(MARKER not in data and b'\x1b[2J' not in data and b'CPU:' in data for data in idle.values())
            finally:
                os.kill(producer_pid, signal.SIGCONT)
            # One failed activity query must recover without repainting body text.
            injection.write_text('fail')
            failed = drain(fds, 2.2)
            assert not injection.exists()
            assert all(MARKER not in data and b'\x1b[2J' not in data and b'CPU:' in data for data in failed.values())
            # Detach the first client after the snapshot but before refresh.
            names = tmux(inner, 'list-clients', '-F', '#{client_pid}\t#{client_name}')
            target = dict(line.split('\t') for line in names.splitlines())[str(clients[0][0].pid)]
            injection.write_text(target)
            fds = [second, nested]
            raced = drain(fds, 2.2)
            assert not injection.exists() and clients[0][0].poll() is not None
            assert all(MARKER not in data and b'\x1b[2J' not in data and b'CPU:' in data for data in raced.values())
            # Monitor recovery must also use status-only publication.
            prior = tmux(inner, 'display', '-p', '#{TMUX_STATUS_COLLECTOR_PID}')
            os.kill(int(prior), signal.SIGKILL)
            recovered = drain(fds, 4)
            assert prior != tmux(inner, 'display', '-p', '#{TMUX_STATUS_COLLECTOR_PID}')
            assert all(MARKER not in data and b'\x1b[2J' not in data and b'CPU:' in data and len(set(re.findall(rb'[A-Z]{40}', data))) >= 2 for data in recovered.values())
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
            assert MARKER in control[second], 'test failed to detect old failure mechanism'
            print('PASS: one query per sample; query failure + snapshot detach + crash recovery preserve body; two clients + nested client update; stable body retained; output live; hidden data; positive control')
        finally:
            for sock in reversed(sockets):
                with contextlib.suppress(subprocess.CalledProcessError): tmux(sock, 'kill-server')
            for proc, fd in clients:
                with contextlib.suppress(subprocess.TimeoutExpired): proc.wait(timeout=3)
                if proc.poll() is None: proc.kill(); proc.wait()
                os.close(fd)

if __name__ == '__main__': main()
