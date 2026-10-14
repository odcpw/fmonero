#!/usr/bin/env python3
"""Exercise the real uninstaller and an already-built CLI, entirely in scratch state.

Run: python3 tests/uninstall-guards.py --binary target/debug/fmonero
No builds, wallet RPC, host systemctl, or installed desktop changes. The PATH traps
are an additional isolation check; the real CLI uses FMONERO_SYSTEMCTL_FIXTURE.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
args = argparse.ArgumentParser(description=__doc__)
args.add_argument('--binary', required=True, type=Path)
options, remainder = args.parse_known_args()
BINARY = options.binary.resolve(strict=True)


class UninstallGuards(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='fmonero-uninstall-')
        self.root = Path(self.temp.name)
        self.env = os.environ.copy()
        self.env.pop('FMONERO_WALLET_PASSWORD', None)
        mapping = {'FMONERO_STATE_DIR': 'state', 'FMONERO_CONFIG_DIR': 'config',
                   'FMONERO_CACHE_DIR': 'cache', 'FMONERO_SYSTEMD_USER_DIR': 'systemd',
                   'XDG_DATA_HOME': 'data', 'XDG_CONFIG_HOME': 'xdg-config',
                   'XDG_STATE_HOME': 'xdg-state', 'XDG_CACHE_HOME': 'xdg-cache',
                   'XDG_RUNTIME_DIR': 'runtime', 'FMONERO_PREFIX': 'bin'}
        for key, name in mapping.items():
            path = self.root / name
            path.mkdir(mode=0o700)
            self.env[key] = str(path)
        self.state = self.root / 'state'
        self.fixture = self.root / 'systemctl.txt'
        self.fixture.write_text('fmonero-wallet-rpc.service active disabled yes -\n'
                                'fmonero-monerod.service active disabled yes -\n')
        self.env['FMONERO_SYSTEMCTL_FIXTURE'] = str(self.fixture)
        self.env['FMONERO_NODES_URL'] = 'http://127.0.0.1:1'
        self.env['GUARD_TEST_ROOT'] = str(self.root)
        shims = self.root / 'shims'
        shims.mkdir()
        self.env['PATH'] = str(shims) + os.pathsep + os.environ['PATH']
        self.script(shims / 'systemctl', """from pathlib import Path
import os, sys
Path(os.environ['GUARD_TEST_ROOT'], 'HOST_SYSTEMCTL_CALLED').touch()
sys.exit(91)
""")
        self.script(shims / 'omarchy', """from pathlib import Path
import fcntl, json, os, sys
root = Path(os.environ['GUARD_TEST_ROOT'])
if sys.argv[1:] == ['plugin', 'list']:
    print('oliver.monero enabled')
elif sys.argv[1:] == ['plugin', 'disable', 'oliver.monero']:
    # At the first desktop mutation, both actual kernel leases must be held.
    blocked = []
    for name in ['send.lock', 'app-session.lock']:
        with (root / 'state' / name).open('r+b') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                blocked.append(False)
            except BlockingIOError:
                blocked.append(True)
    (root / 'desktop-action.json').write_text(json.dumps(blocked))
else:
    sys.exit(92)
""")
        self.installed = self.root / 'bin/fmonero'
        self.installed.symlink_to(BINARY)
        for unit in ['fmonero-monerod.service', 'fmonero-wallet-rpc.service']:
            (self.root / 'systemd' / unit).write_text('scratch unit marker\n')
        desktop = self.root / 'data/applications'
        desktop.mkdir()
        self.desktop = desktop / 'fmonero.desktop'
        self.desktop.write_text('scratch desktop marker\n')
        self.wallets = self.root / 'data/fmonero/wallets/mainnet'
        self.wallets.mkdir(parents=True)
        self.wallet_marker = self.wallets / 'not-a-wallet.sentinel'
        self.wallet_marker.write_text('must survive\n')
        self.chain = self.root / 'chain'
        self.chain.mkdir()
        (self.chain / 'not-a-chain.sentinel').write_text('must survive\n')
        self.fm('settings', 'set', 'wallet.dir', str(self.wallets))
        self.fm('settings', 'set', 'node.data_dir', str(self.chain))
        for name in ['send.lock', 'app-session.lock', '.lock']:
            (self.state / name).touch(mode=0o600)
        (self.state / 'app-stop-generation.json').write_text('7')
        (self.state / 'disposable.txt').write_text('disposable\n')
        (self.root / 'cache/disposable.txt').write_text('disposable\n')

    def tearDown(self):
        self.assertFalse((self.root / 'HOST_SYSTEMCTL_CALLED').exists())
        self.assertEqual(self.wallet_marker.read_text(), 'must survive\n')
        self.assertEqual((self.chain / 'not-a-chain.sentinel').read_text(), 'must survive\n')
        self.temp.cleanup()

    def script(self, path, body):
        path.write_text('#!' + sys.executable + '\n' + body)
        path.chmod(0o700)

    def fm(self, *arguments):
        result = subprocess.run([str(BINARY), *arguments, '--json'], env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def uninstall(self, *arguments, expected=3):
        result = subprocess.run(['bash', str(ROOT / 'scripts/uninstall.sh'), '--yes', '--no-color',
                                 *arguments], env=self.env, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def unchanged(self, generation='7'):
        self.assertTrue(self.installed.exists())
        self.assertTrue(self.desktop.exists())
        self.assertTrue((self.root / 'config/config.toml').exists())
        self.assertEqual((self.state / 'app-stop-generation.json').read_text(), generation)
        self.assertFalse((self.root / 'desktop-action.json').exists())
        for unit in ['fmonero-monerod.service', 'fmonero-wallet-rpc.service']:
            self.assertEqual((self.root / 'systemd' / unit).read_text(), 'scratch unit marker\n')

    def interpose_stop(self, body):
        self.installed.unlink()
        self.script(self.installed, f"""import os, pathlib, subprocess, sys
real = {str(BINARY)!r}
root = pathlib.Path(os.environ['GUARD_TEST_ROOT'])
if sys.argv[1:3] == ['app', 'stop']:
{body}
os.execv(real, [real, *sys.argv[1:]])
""")

    def test_both_pending_names_refuse_plain_corrupt_unreadable_and_symlink_records(self):
        for name in ['pending.json', 'native-pending.json']:
            for kind in ['corrupt', 'unreadable', 'symlink', 'directory', 'fifo']:
                with self.subTest(name=name, kind=kind):
                    path = self.state / name
                    if kind == 'symlink':
                        path.symlink_to(self.root / 'absent-record')
                    elif kind == 'directory':
                        path.mkdir()
                    elif kind == 'fifo':
                        os.mkfifo(path, 0o600)
                    else:
                        path.write_text('not valid JSON')
                        if kind == 'unreadable':
                            path.chmod(0)
                    before = self.fixture.read_bytes()
                    self.uninstall()
                    self.uninstall('--purge')
                    self.assertEqual(self.fixture.read_bytes(), before)
                    self.unchanged()
                    if kind == 'directory':
                        path.rmdir()
                    else:
                        if kind == 'unreadable':
                            path.chmod(0o600)
                        path.unlink()

    def test_held_money_lock_refuses_before_app_stop(self):
        with (self.state / 'send.lock').open('r+b') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.uninstall('--purge')
            self.unchanged()
            self.assertIn('active disabled', self.fixture.read_text())

    def test_symlink_lock_is_refused_without_following_it(self):
        lock = self.state / 'send.lock'
        lock.unlink()
        marker = self.root / 'not-a-lock'
        marker.write_text('untouched')
        lock.symlink_to(marker)
        self.uninstall()
        self.assertEqual(marker.read_text(), 'untouched')
        self.unchanged()

    def test_app_stop_safety_failure_is_not_bypassed(self):
        self.interpose_stop('    sys.exit(3)\n')
        before = self.fixture.read_bytes()
        self.uninstall('--purge')
        self.assertEqual(self.fixture.read_bytes(), before)
        self.unchanged()

    def test_record_appearing_after_guarded_stop_prevents_cleanup(self):
        self.interpose_stop("""    result = subprocess.run([real, *sys.argv[1:]], env=os.environ)
    if result.returncode == 0:
        (root / 'state/native-pending.json').write_text('uncertainty must survive')
    sys.exit(result.returncode)
""")
        self.uninstall('--purge')
        self.unchanged(generation='8')
        self.assertEqual((self.state / 'native-pending.json').read_text(), 'uncertainty must survive')

    def test_money_lock_acquired_after_stop_prevents_cleanup(self):
        self.interpose_stop("""    import time
    result = subprocess.run([real, *sys.argv[1:]], env=os.environ)
    if result.returncode != 0:
        sys.exit(result.returncode)
    (root / 'acquire-now').touch()
    deadline = time.monotonic() + 3
    while not (root / 'acquired').exists():
        if time.monotonic() > deadline:
            sys.exit(93)
        time.sleep(.01)
    sys.exit(0)
""")
        release = threading.Event()

        def hold_after_stop():
            deadline = time.monotonic() + 8
            while not (self.root / 'acquire-now').exists():
                if release.wait(.01) or time.monotonic() > deadline:
                    return
            with (self.state / 'send.lock').open('r+b') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                (self.root / 'acquired').touch()
                release.wait(10)

        actor = threading.Thread(target=hold_after_stop)
        actor.start()
        try:
            self.uninstall('--purge')
            self.assertTrue((self.root / 'acquired').exists())
            self.unchanged(generation='8')
        finally:
            release.set()
            actor.join(timeout=5)
        self.assertFalse(actor.is_alive())

    def test_purge_keeps_stable_lock_inodes_and_shutdown_generation(self):
        before = {name: (self.state / name).stat().st_ino
                  for name in ['send.lock', 'app-session.lock', '.lock']}
        self.uninstall('--purge', expected=0)
        self.assertFalse(self.installed.exists())
        self.assertFalse(self.desktop.exists())
        self.assertFalse((self.root / 'config').exists())
        self.assertFalse((self.root / 'cache').exists())
        self.assertFalse((self.state / 'disposable.txt').exists())
        self.assertEqual((self.state / 'app-stop-generation.json').read_text(), '8')
        self.assertEqual(json.loads((self.root / 'desktop-action.json').read_text()), [True, True])
        for name, inode in before.items():
            self.assertEqual((self.state / name).stat().st_ino, inode)
        # Repeated uninstall with no binary never guesses at old settings or resets leases.
        self.uninstall('--purge', expected=0)
        self.assertEqual((self.state / 'app-stop-generation.json').read_text(), '8')
        for name, inode in before.items():
            self.assertEqual((self.state / name).stat().st_ino, inode)

    def test_missing_binary_refuses_active_service_or_frontend(self):
        self.installed.unlink()
        self.uninstall()
        self.assertTrue(self.desktop.exists())
        self.fixture.write_text('')
        with (self.state / 'app-session.lock').open('r+b') as lock:
            fcntl.flock(lock, fcntl.LOCK_SH | fcntl.LOCK_NB)
            self.uninstall()
            self.assertTrue(self.desktop.exists())

    def test_missing_binary_can_remove_inactive_integration_but_retains_unverified_data(self):
        self.installed.unlink()
        self.fixture.write_text('')
        self.uninstall('--purge', expected=0)
        self.assertFalse(self.desktop.exists())
        self.assertTrue((self.root / 'config/config.toml').exists())
        self.assertTrue((self.root / 'cache/disposable.txt').exists())
        self.assertTrue((self.state / 'disposable.txt').exists())
        self.assertEqual((self.state / 'app-stop-generation.json').read_text(), '7')

    def test_configured_wallet_below_purge_directory_is_preserved(self):
        nested = self.root / 'config/custom-wallets'
        nested.mkdir()
        sentinel = nested / 'not-a-wallet.sentinel'
        sentinel.write_text('keep')
        self.fm('settings', 'set', 'wallet.dir', str(nested))
        self.uninstall('--purge')
        self.unchanged()
        self.assertEqual(sentinel.read_text(), 'keep')

    def test_dry_run_does_not_stop_services_or_replace_state(self):
        before = self.fixture.read_bytes()
        inodes = {name: (self.state / name).stat().st_ino
                  for name in ['send.lock', 'app-session.lock', 'app-stop-generation.json']}
        self.uninstall('--purge', '--dry-run', expected=0)
        self.unchanged()
        self.assertEqual(self.fixture.read_bytes(), before)
        for name, inode in inodes.items():
            self.assertEqual((self.state / name).stat().st_ino, inode)


if __name__ == '__main__':
    unittest.main(argv=[sys.argv[0], *remainder])
