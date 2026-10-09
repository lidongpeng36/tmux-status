"""Real-server singleton, fast startup, config reload, crash recovery and isolation."""
import fcntl
import json
import os
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

BINARY = os.path.realpath(os.environ.get('TMUX_STATUS_BIN', 'target/release/tmux-status'))
PLUGIN = os.path.realpath('tmux-status.tmux')

class Server:
    def __init__(self, directory, name):
        self.socket = os.path.join(directory, name)
        self.config = self.socket + '.conf'
        self.clients = []
        with open(self.config, 'w') as f:
            f.write('set -g status-right-length 150\nset -g status-interval 1\n'
                    'set -g @tmux-status-network off\nset -g @tmux-status-interval 1\n'
                    'set -g @tmux-status-bin "' + BINARY + '"\n'
                    "set -g status-right '#{tmux_status}'\n"
                    "run-shell '" + PLUGIN + "'\n")
        self.command('-f', self.config, 'new-session', '-d', '-s', 'one', 'sleep 180')
        self.pid = self.command('display-message', '-p', '#{pid}').strip()
    def command(self, *args):
        return subprocess.check_output(['tmux', '-S', self.socket, *args], text=True,
                                       stderr=subprocess.STDOUT, timeout=10)
    def option(self, option):
        return self.command('show-option', '-gqv', option).strip()
    def attach(self, session='one'):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 180, 0, 0))
        process = subprocess.Popen(['tmux', '-S', self.socket, 'attach-session', '-t', session],
                                   stdin=slave, stdout=slave, stderr=slave,
                                   env={**os.environ, 'TERM': 'xterm-256color'})
        os.close(slave)
        self.clients.append((process, master))
    def drain(self):
        for process, master in self.clients:
            if process.poll() is None and select.select([master], [], [], 0)[0]:
                os.read(master, 65536)
    def wait(self, condition, timeout=5):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.drain()
            if condition(): return
            time.sleep(.05)
        raise AssertionError('condition timed out for ' + self.socket)
    def reload(self):
        self.command('source-file', self.config)
    def close(self):
        try: self.command('kill-server')
        except subprocess.CalledProcessError: pass
        for process, master in self.clients:
            try: process.wait(timeout=3)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
            os.close(master)
        self.clients.clear()

def alive(pid):
    try: os.kill(int(pid), 0); return True
    except ProcessLookupError: return False

def main():
    with tempfile.TemporaryDirectory(prefix='tmux-status-servers-') as directory:
        servers = []
        try:
            start = time.monotonic()
            first = Server(directory, 'first'); servers.append(first)
            first.wait(lambda: bool(first.option('@tmux-status-json')))
            startup = time.monotonic() - start
            first.wait(lambda: json.loads(first.option('@tmux-status-json'))['cpu_percent'] is not None)
            assert startup < 2, startup
            owner = first.option('@tmux-status-collector-pid')
            first.attach()
            first.command('new-session', '-d', '-s', 'two', 'sleep 180')
            first.attach('two')
            for _ in range(4): first.reload()
            time.sleep(1); first.drain()
            assert first.option('@tmux-status-collector-pid') == owner
            hooks = first.command('show-hooks', '-g', 'client-attached')
            assert hooks.count('tmux-status-start-hook') == 1, hooks
            # Two sessions/clients still consume the same native snapshot.
            assert '#(' not in first.option('status-right')
            assert first.option('status-right') == '#{@tmux-status-local}'
            rendered = first.command('display-message', '-p', '#{T:status-right}')
            assert '%#[default]' in rendered, rendered
            assert ' ' in first.option('@tmux-status-local')
            assert ' ' in first.option('@tmux-status-remote')
            # Partial appearance overrides apply without resetting the sampler.
            first.command('set', '-g', '@tmux-status-cpu-label', 'C: ', ';',
                          'set', '-g', '@tmux-status-appearance', '{"normal_color":"colour39","warning_color":"colour39","critical_color":"colour39","mem_template":"{label}{used}/{total}","separator":" | "}')
            first.command('run-shell', PLUGIN)
            first.wait(lambda: 'C: ' in first.option('@tmux-status-local'))
            assert first.option('@tmux-status-collector-pid') == owner
            assert 'colour39' in first.option('@tmux-status-local')
            first.command('set', '-gu', '@tmux-status-cpu-label', ';', 'set', '-gu', '@tmux-status-appearance')
            first.command('run-shell', PLUGIN)
            first.wait(lambda: ' ' in first.option('@tmux-status-local'))
            # Reload changes configuration via IPC, preserving owner and baseline.
            first.command('set', '-g', '@tmux-status-network', 'on', ';',
                          'set', '-g', '@tmux-status-check-urls', 'http://127.0.0.1:1/', ';',
                          'set', '-g', '@tmux-status-network-direct', 'on')
            first.command('run-shell', PLUGIN)
            first.wait(lambda: json.loads(first.option('@tmux-status-json'))['reachability'] == 'unreachable')
            assert first.option('@tmux-status-collector-pid') == owner
            second = Server(directory, 'second'); servers.append(second)
            second.wait(lambda: bool(second.option('@tmux-status-collector-pid')))
            other = second.option('@tmux-status-collector-pid')
            assert other != owner
            # A killed owner leaves no permanent lock. Reload restores exactly one owner.
            os.kill(int(owner), signal.SIGKILL)
            first.wait(lambda: not alive(owner))
            first.reload()
            first.wait(lambda: first.option('@tmux-status-collector-pid') not in ('', owner))
            replacement = first.option('@tmux-status-collector-pid')
            first.close()
            first.wait(lambda: not alive(replacement))
            assert alive(other), 'first server shutdown killed another server collector'
            second.close()
            second.wait(lambda: not alive(other))
            runtime = os.path.join(tempfile.gettempdir(), 'tmux-status-' + str(os.getuid()))
            assert not os.path.exists(os.path.join(runtime, first.pid + '.sock'))
            assert not os.path.exists(os.path.join(runtime, second.pid + '.sock'))
            print(json.dumps({'startup_seconds': round(startup, 3), 'multi_client_singleton': True,
                              'reload_preserved_owner': True, 'live_config_update': True,
                              'crash_recovery': True, 'two_servers_isolated': True, 'clean_shutdown': True}))
        finally:
            for server in servers: server.close()

if __name__ == '__main__': main()
