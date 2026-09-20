#!/usr/bin/env python3
"""Paste a fixture address through a real terminal; never open a wallet or build a payment."""
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import time

repo = Path(__file__).resolve().parents[1]
address = re.search(r'const DEST: &str = "([^"]+)";', (repo / 'crates/fmonero/src/tui/tests.rs').read_text()).group(1)
with tempfile.TemporaryDirectory(prefix='fmonero-paste-') as directory:
    root = Path(directory)
    env = os.environ.copy()
    for key, name in {
        'FMONERO_STATE_DIR': 'state', 'FMONERO_CONFIG_DIR': 'config',
        'FMONERO_CACHE_DIR': 'cache', 'XDG_DATA_HOME': 'data',
        'FMONERO_SYSTEMD_USER_DIR': 'systemd',
    }.items():
        path = root / name
        path.mkdir(mode=0o700)
        env[key] = str(path)
    fixture = root / 'systemctl.txt'
    fixture.write_text('')
    env['FMONERO_SYSTEMCTL_FIXTURE'] = str(fixture)
    env['FMONERO_NODES_URL'] = 'http://127.0.0.1:1'
    env['TERM'] = 'xterm-256color'
    env.pop('FMONERO_WALLET_PASSWORD', None)
    binary = str(repo / 'target/release/fmonero')
    # Refused loopback connections, independent of the owner's live services.
    for key, value in [('node.url', 'http://127.0.0.1:1'), ('wallet.url', 'http://127.0.0.1:2')]:
        subprocess.run([binary, 'settings', 'set', key, value, '--json'], env=env,
                       check=True, stdout=subprocess.DEVNULL, timeout=5)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 180, 0, 0))
    child = subprocess.Popen([binary, 'tui', '--send'], env=env, stdin=slave,
                             stdout=slave, stderr=slave, start_new_session=True,
                             preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
    os.close(slave)
    output = bytearray()

    def await_output(needle):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if select.select([master], [], [], .1)[0]:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    break
            if needle in output or needle in re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', output):
                return
            if child.poll() is not None:
                break
        raise AssertionError('terminal did not show the expected paste state')

    try:
        await_output(b'\x1b[?2004h')
        # Include a newline as copied addresses often have one. It must not act as Enter.
        os.write(master, b'\x1b[200~' + address.encode() + b'\n\x1b[201~')
        await_output(address.encode())
        assert child.poll() is None
        assert not (root / 'state/pending.json').exists()
        os.write(master, b'\x03')
        child.wait(timeout=5)
        assert child.returncode == 130
        print('PASS: terminal paste displays the complete recipient; no form submission or payment.')
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        os.close(master)
