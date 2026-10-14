"""Real tmux hooks/PTY focus, no-client pause, network pause, and wake latency."""
import contextlib,fcntl,http.server,json,os,pathlib,pty,select,shlex,signal,struct,subprocess,tempfile,termios,threading,time
ROOT=pathlib.Path(__file__).resolve().parent.parent
BIN=pathlib.Path(os.environ.get('TMUX_STATUS_BIN',ROOT/'target/release/tmux-status')).resolve()
requests=[]
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  requests.append(time.monotonic());self.send_response(204);self.end_headers()
 def log_message(self,*args):pass
http=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=http.serve_forever,daemon=True).start()
with tempfile.TemporaryDirectory(prefix='status-adaptive-') as tmp:
 d=pathlib.Path(tmp);sock=str(d/'socket');clients=[]
 def t(*args):return subprocess.check_output(['tmux','-S',sock,*args],text=True,stderr=subprocess.STDOUT,timeout=8).strip()
 def value(key):return t('display','-p','#{'+key+'}')
 def drain():
  fds=[fd for proc,fd in clients if proc.poll() is None]
  for fd in select.select(fds,[],[],0)[0]:
   with contextlib.suppress(OSError):os.read(fd,65536)
 def wait(fn,seconds=5):
  end=time.monotonic()+seconds
  while time.monotonic()<end:
   drain()
   result=fn()
   if result:return result
   time.sleep(.05)
  raise AssertionError('timeout: '+value('TMUX_STATUS_POLICY'))
 def sleep(seconds):
  end=time.monotonic()+seconds
  while time.monotonic()<end:drain();time.sleep(.05)
 def attach():
  m,s=pty.openpty();fcntl.ioctl(s,termios.TIOCSWINSZ,struct.pack('HHHH',24,140,0,0))
  p=subprocess.Popen(['tmux','-S',sock,'attach'],stdin=s,stdout=s,stderr=s,env={**os.environ,'TERM':'xterm-256color','TMUX':''});os.close(s);clients.append((p,m));return p,m
 conf=d/'conf'
 conf.write_text('set -g focus-events on\nset -as terminal-features ",xterm-256color:focus"\nset -g status-right "#{tmux_status}"\nset -g @tmux-status-interval 1\nset -g @tmux-status-network-interval 1\nset -g @tmux-status-network-direct on\nset -g @tmux-status-check-urls "http://127.0.0.1:'+str(http.server_port)+'/"\nset -g @tmux-status-bin '+shlex.quote(str(BIN))+'\nrun-shell '+shlex.quote(str(ROOT/'tmux-status.tmux'))+'\n')
 try:
  t('-f',str(conf),'new','-d','sleep 240')
  wait(lambda:value('TMUX_STATUS_STATE')=='ok')
  worker=value('TMUX_STATUS_COLLECTOR_PID');monitor=value('TMUX_STATUS_SUPERVISOR_PID')
  wait(lambda:value('TMUX_STATUS_STATE')=='paused',seconds=35)
  # Allow the one bounded in-flight HTTP request to finish, then observe quiet.
  sleep(.5);count=len(requests);assert count>0
  sleep(16)
  assert len(requests)==count,'network continued while paused'
  assert value('TMUX_STATUS_STATE')=='paused' and value('TMUX_STATUS_COLLECTOR_PID')==worker
  assert value('TMUX_STATUS_INTERVAL')=='0'
  # A paused monitor sleeps longer, but child exit must still wake it promptly.
  old_worker=worker;start=time.monotonic();os.kill(int(worker),signal.SIGKILL)
  worker=wait(lambda:(p if (p:=value('TMUX_STATUS_COLLECTOR_PID')) not in ('',old_worker) else None),seconds=4)
  paused_crash_latency=time.monotonic()-start
  assert value('TMUX_STATUS_SUPERVISOR_PID')==monitor
  wait(lambda:value('TMUX_STATUS_STATE')=='paused',seconds=35)
  start=time.monotonic();client,fd=attach()
  wait(lambda:value('TMUX_STATUS_STATE')=='ok' and json.loads(value('TMUX_STATUS_JSON'))['cpu_percent'] is not None,seconds=3)
  latency=time.monotonic()-start;assert value('TMUX_STATUS_COLLECTOR_PID')==worker
  wait(lambda:len(requests)>count)
  sleep(6);os.write(fd,b'\x1b[I');sleep(.2);os.write(fd,b'\x1b[O')
  wait(lambda:'focused' not in t('list-clients','-F','#{client_flags}'))
  wait(lambda:value('TMUX_STATUS_POLICY')=='background',seconds=35)
  assert int(value('TMUX_STATUS_INTERVAL'))>=30 and int(value('TMUX_STATUS_NETWORK_INTERVAL'))>=300
  start=time.monotonic();os.write(fd,b'\x1b[I')
  wait(lambda:value('TMUX_STATUS_POLICY') not in ['background','paused'],seconds=3)
  focus_latency=time.monotonic()-start
  # One other focused client keeps the server active.
  other,otherfd=attach();sleep(.5);os.write(otherfd,b'\x1b[I');os.write(fd,b'\x1b[O');sleep(31)
  assert value('TMUX_STATUS_POLICY') not in ['background','paused']
  # Unknown focus capability must also remain active (never inferred hidden).
  t('set','-g','focus-events','off');os.write(otherfd,b'\x1b[O')
  subprocess.run([str(BIN),'--wake','--serve',sock,'--server-pid',value('pid')],check=True)
  assert value('TMUX_STATUS_SUPERVISOR_PID')==monitor
  t('set','-g','@tmux-status-adaptive','off');t('run-shell',str(ROOT/'tmux-status.tmux'))
  wait(lambda:value('TMUX_STATUS_POLICY')=='fixed')
  assert value('TMUX_STATUS_INTERVAL')=='1'
  hooks=t('show-hooks','-g','client-attached')
  assert hooks.count('tmux-status-wake-hook')==1 and hooks.count('tmux-status-start-hook')==1
  print(json.dumps({'paused_network_requests':0,'paused_crash_recovery_seconds':round(paused_crash_latency,3),'paused_no_false_restart':True,'attach_resume_seconds':round(latency,3),'focus_resume_seconds':round(focus_latency,3),'all_unfocused_throttled':True,'any_focused_keeps_active':True}))
 finally:
  with contextlib.suppress(subprocess.CalledProcessError):t('kill-server')
  for p,fd in clients:
   with contextlib.suppress(subprocess.TimeoutExpired):p.wait(timeout=3)
   if p.poll() is None:p.kill();p.wait()
   os.close(fd)
  http.shutdown();http.server_close()
