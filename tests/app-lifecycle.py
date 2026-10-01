#!/usr/bin/env python3
"""Real TUI windows must disappear on app stop; all services/configuration are isolated."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

binary = Path(__file__).resolve().parents[1] / 'target/release/fmonero'
with tempfile.TemporaryDirectory(prefix='fmonero-app-lifecycle-') as directory:
    root = Path(directory)
    env = os.environ.copy()
    for key, name in {'FMONERO_STATE_DIR': 'state', 'FMONERO_CONFIG_DIR': 'config',
                      'FMONERO_CACHE_DIR': 'cache', 'XDG_DATA_HOME': 'data',
                      'FMONERO_SYSTEMD_USER_DIR': 'systemd'}.items():
        path = root / name
        path.mkdir(mode=0o700)
        env[key] = str(path)
    fixture = root / 'systemctl.txt'
    fixture.write_text('fmonero-wallet-rpc.service inactive disabled yes 0\nfmonero-monerod.service inactive disabled yes 0\n')
    env['FMONERO_SYSTEMCTL_FIXTURE'] = str(fixture)
    env['FMONERO_NODES_URL'] = 'http://127.0.0.1:1'
    env['TERM'] = 'xterm-256color'
    env.pop('FMONERO_WALLET_PASSWORD', None)

    def fm(*args):
        result = subprocess.run([str(binary), *args, '--json'], env=env, text=True,
                                capture_output=True, timeout=10, check=True)
        return json.loads(result.stdout)

    for key, value in [('node.url', 'http://127.0.0.1:1'), ('wallet.url', 'http://127.0.0.1:2')]:
        fm('settings', 'set', key, value)
    windows = []

    def open_window():
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 34, 110, 0, 0))
        child = subprocess.Popen([str(binary), 'tui', '--wallet'], env=env, stdin=slave,
                                 stdout=slave, stderr=slave, start_new_session=True,
                                 preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(slave)
        windows.append((child, master))
        data = bytearray()
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline and child.poll() is None:
            if select.select([master], [], [], .05)[0]:
                data.extend(os.read(master, 65536))
                if b'\x1b[?2004h' in data:
                    return child, master
        raise AssertionError('TUI did not open')

    try:
        assert not fm('app', 'status')['running']
        first, _ = open_window()
        second, _ = open_window()
        assert fm('app', 'status')['frontend_running']
        assert fm('app', 'stop', '--yes')['stopped']
        first.wait(timeout=6)
        second.wait(timeout=6)
        assert first.returncode == second.returncode == 0
        assert not fm('app', 'status')['running']
        later, terminal = open_window()
        time.sleep(.15)
        assert later.poll() is None and fm('app', 'status')['running']
        os.write(terminal, b'\x03')
        later.wait(timeout=5)
        assert later.returncode == 130
        assert not fm('app', 'status')['running']
        print('PASS: inactive before launch; two TUIs closed by power; reopen succeeds; inactive after exit.')
    finally:
        for child, terminal in windows:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            os.close(terminal)
