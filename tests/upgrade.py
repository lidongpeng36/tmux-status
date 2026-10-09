"""Managed handoff: invalid config keeps old owner, upgrade replaces only ours."""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import time

ROOT=pathlib.Path(__file__).resolve().parent.parent
BIN=pathlib.Path(os.environ.get('TMUX_STATUS_BIN','target/release/tmux-status')).resolve()
OLD=pathlib.Path(os.environ.get('TMUX_STATUS_OLD_BIN',str(BIN))).resolve()

def main():
    with tempfile.TemporaryDirectory(prefix='tmux-status-upgrade-') as directory:
        directory=pathlib.Path(directory)
        plugin=directory/'plugin';plugin.mkdir()
        shutil.copytree(ROOT/'scripts',plugin/'scripts')
        for file in ['VERSION','tmux-status.tmux']:shutil.copy(ROOT/file,plugin/file)
        cache=plugin/'bin';cache.mkdir()
        old=cache/'tmux-status-old-managed';new=cache/'tmux-status-new-managed'
        shutil.copy2(OLD,old);shutil.copy2(BIN,new)
        socket=str(directory/'socket');config=directory/'tmux.conf';config.write_text('set -g status-interval 1\n')
        def tmux(*args):return subprocess.check_output(['tmux','-S',socket,*args],text=True,stderr=subprocess.STDOUT,timeout=5).strip()
        def option(name):return tmux('show-option','-gqv',name)
        def alive(pid):
            try:os.kill(int(pid),0);return True
            except ProcessLookupError:return False
        def wait(check):
            deadline=time.monotonic()+8
            while time.monotonic()<deadline:
                if check():return
                time.sleep(.05)
            raise AssertionError('upgrade condition timed out')
        unrelated=subprocess.Popen(['sleep','90'])
        try:
            tmux('-f',str(config),'new-session','-d','sleep 90')
            server=tmux('display','-p','#{pid}')
            tmux('run-shell','-b','exec "'+str(old)+'" --serve "'+socket+'" --server-pid '+server+' --interval 1')
            wait(lambda:bool(option('@tmux-status-collector-pid')))
            owner=option('@tmux-status-collector-pid')
            tmux('set','-g','@tmux-status-bin',str(new),';','set','-g','@tmux-status-network','off',
                 ';','set','-g','@tmux-status-appearance','{"typo":1}')
            tmux('run-shell',str(plugin/'tmux-status.tmux'));time.sleep(.7)
            assert alive(owner),'bad new config killed healthy old owner'
            tmux('set','-gu','@tmux-status-appearance')
            tmux('run-shell',str(plugin/'tmux-status.tmux'))
            wait(lambda:option('@tmux-status-collector-pid') not in ('',owner))
            replacement=option('@tmux-status-collector-pid')
            wait(lambda:not alive(owner))
            assert alive(unrelated.pid),'upgrade signalled an unrelated process'
            for _ in range(3):tmux('run-shell',str(plugin/'tmux-status.tmux'))
            time.sleep(.7);assert option('@tmux-status-collector-pid')==replacement
            assert ' ' in option('@tmux-status-local')
            print(json.dumps({'invalid_configuration_kept_old_owner':True,'managed_upgrade':True,'unrelated_process_preserved':True,'reload_reuses_new_owner':True}))
        finally:
            try:tmux('kill-server')
            except subprocess.CalledProcessError:pass
            unrelated.terminate();unrelated.wait(timeout=3)

if __name__=='__main__':main()
