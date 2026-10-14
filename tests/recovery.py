"""Transient RPC failure, crash/stall recovery, singleton monitor and shutdown."""
import contextlib,json,os,pathlib,shlex,shutil,signal,subprocess,tempfile,time
ROOT=pathlib.Path(__file__).resolve().parent.parent
BIN=pathlib.Path(os.environ.get('TMUX_STATUS_BIN',ROOT/'target/release/tmux-status')).resolve()
REAL=shutil.which('tmux')
def alive(pid):
 try:os.kill(int(pid),0);return True
 except ProcessLookupError:return False
def wait(fn,seconds=12):
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  value=fn()
  if value:return value
  time.sleep(.05)
 raise AssertionError('timed out')
with tempfile.TemporaryDirectory(prefix='status-recovery-') as tmp:
 d=pathlib.Path(tmp);sock=str(d/'socket');marker=d/'fail';mock=d/'tmux';config=d/'conf'
 # Fail only one list-clients command, after successful first data publication.
 mock.write_text('#!/usr/bin/env python3\nimport os,sys,pathlib\np=pathlib.Path('+repr(str(marker))+')\nif p.exists() and os.environ.get("TMUX_STATUS_WORKER")=="1" and ((p.read_text()=="list" and "list-clients" in sys.argv) or (p.read_text()=="publish" and "TMUX_STATUS_COLLECTOR_VERSION" in sys.argv) or (p.read_text()=="timeout" and "list-clients" in sys.argv)):\n mode=p.read_text();p.unlink()\n if mode=="timeout":\n  import time;time.sleep(10)\n sys.stderr.write("injected query error\\n");sys.exit(1)\nos.execv('+repr(REAL)+',['+repr(REAL)+']+sys.argv[1:])\n');mock.chmod(0o755)
 def t(*args):return subprocess.check_output([REAL,'-S',sock,*args],text=True,stderr=subprocess.STDOUT,timeout=8).strip()
 def value(key):return t('display','-p','#{'+key+'}')
 config.write_text('set -g @tmux-status-adaptive off\nset -g @tmux-status-network off\nset -g @tmux-status-interval 1\nset -g @tmux-status-bin '+shlex.quote(str(BIN))+'\n')
 monitor=worker=None
 try:
  t('-f',str(config),'new','-d','sleep 180');server=value('pid')
  launch=shlex.join([str(ROOT/'scripts/start.sh'),sock,server]);marker.write_text("list")
  t('set-environment','-g','TMUX_BIN',str(mock));t('run-shell','-b',launch)
  worker=wait(lambda:value('TMUX_STATUS_COLLECTOR_PID'))
  monitor=wait(lambda:value('TMUX_STATUS_SUPERVISOR_PID'))
  wait(lambda:json.loads(value('TMUX_STATUS_JSON') or '{}').get('cpu_percent') is not None)
  assert alive(worker) and value('TMUX_STATUS_COLLECTOR_PID')==worker
  log=pathlib.Path(value('TMUX_STATUS_LOG'));assert 'injected query error' in log.read_text()
  marker.write_text('publish')
  wait(lambda:not marker.exists())
  wait(lambda:'publish snapshot' in log.read_text())
  time.sleep(1.2)
  assert alive(worker) and value('TMUX_STATUS_COLLECTOR_PID')==worker
  marker.write_text('timeout')
  wait(lambda:'exceeded 3 seconds' in log.read_text())
  assert alive(worker) and value('TMUX_STATUS_COLLECTOR_PID')==worker
  # No reload or attach: the monitor must restart the killed worker by itself.
  os.kill(int(worker),signal.SIGKILL)
  worker=wait(lambda:(p if (p:=value('TMUX_STATUS_COLLECTOR_PID')) not in ('',worker) and alive(p) else None))
  assert value('TMUX_STATUS_SUPERVISOR_PID')==monitor
  for _ in range(3):t('run-shell',launch)
  assert value('TMUX_STATUS_SUPERVISOR_PID')==monitor and value('TMUX_STATUS_COLLECTOR_PID')==worker
  # A stopped worker is stale even though kill(pid,0) still succeeds.
  os.kill(int(worker),signal.SIGSTOP)
  wait(lambda:value('TMUX_STATUS_STATE')=='restarting',seconds=20)
  assert 'STALE' in value('TMUX_STATUS_LOCAL')
  prior=worker;worker=wait(lambda:(p if (p:=value('TMUX_STATUS_COLLECTOR_PID')) not in ('',prior) and alive(p) else None))
  wait(lambda:json.loads(value('TMUX_STATUS_JSON')).get('cpu_percent') is not None)
  assert 'heartbeat expired' in log.read_text()
  # When the final tmux session exits, neither process may survive it.
  t('kill-server');wait(lambda:not alive(worker) and not alive(monitor),seconds=5)
  print('PASS: publication and query RPC retry; SIGKILL restart without attach; stalled-worker expiry/STALE; reload singleton; server exit cleanup')
 finally:
  with contextlib.suppress(subprocess.CalledProcessError):t('kill-server')

# Repeated immediate failures must back off, without relying on attach events.
with tempfile.TemporaryDirectory(prefix='status-backoff-') as tmp:
 d=pathlib.Path(tmp);sock=str(d/'socket');fake=d/'failing-worker'
 fake.write_text('#!/usr/bin/env python3\nimport os,sys\nif "--check-config" in sys.argv or "--supervise" in sys.argv:\n os.execv('+repr(str(BIN))+',['+repr(str(BIN))+']+sys.argv[1:])\nsys.exit(7)\n');fake.chmod(0o755)
 def t(*a):return subprocess.check_output([REAL,'-S',sock,*a],text=True,stderr=subprocess.STDOUT,timeout=8).strip()
 try:
  t('-f','/dev/null','new','-d','sleep 60');server=t('display','-p','#{pid}')
  t('set','-g','@tmux-status-adaptive','off');t('set','-g','@tmux-status-bin',str(fake));t('set','-g','@tmux-status-network','off')
  t('run-shell','-b',shlex.join([str(ROOT/'scripts/start.sh'),sock,server]))
  log=pathlib.Path(wait(lambda:t('display','-p','#{TMUX_STATUS_LOG}')))
  began=time.monotonic()
  wait(lambda:'retry in 4s' in log.read_text(),seconds=12)
  assert time.monotonic()-began>=2.5,'child signals bypassed retry delay'
  text=log.read_text();assert 'status=exit status: 7' in text
  assert 'retry in 1s' in text and 'retry in 2s' in text
  assert 'STALE' in t('display','-p','#{TMUX_STATUS_LOCAL}')
  print('PASS: persistent failure logs exit status; 1/2/4 second exponential backoff; stale indicator')
 finally:
  with contextlib.suppress(subprocess.CalledProcessError):t('kill-server')
