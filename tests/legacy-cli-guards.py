#!/usr/bin/env python3
"""Run the real legacy CLI branches against isolated existing protocol fixtures.

python3 tests/legacy-cli-guards.py --binary target/debug/fmonero --mock target/debug/fmonero-mock
No build, native wallet, public network, owner service, or broadcast is used.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import select
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', required=True, type=Path)
parser.add_argument('--mock', required=True, type=Path)
options, remainder = parser.parse_known_args()
BINARY = options.binary.resolve(strict=True)
MOCK = options.mock.resolve(strict=True)
TXID = 'a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f'


class LegacyGuards(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='fmonero-legacy-guards-')
        self.root = Path(self.temp.name)
        self.env = os.environ.copy()
        self.env.pop('FMONERO_WALLET_PASSWORD', None)
        for key, name in {'FMONERO_STATE_DIR': 'state', 'FMONERO_CONFIG_DIR': 'config',
                          'FMONERO_CACHE_DIR': 'cache', 'FMONERO_SYSTEMD_USER_DIR': 'systemd',
                          'XDG_DATA_HOME': 'data'}.items():
            path = self.root / name
            path.mkdir(mode=0o700)
            self.env[key] = str(path)
        self.fixture = self.root / 'systemctl.txt'
        self.fixture.write_text('fmonero-wallet-rpc.service active disabled yes -\n')
        self.env['FMONERO_SYSTEMCTL_FIXTURE'] = str(self.fixture)
        self.env['FMONERO_NODES_URL'] = 'http://127.0.0.1:1'
        shims = self.root / 'shims'
        shims.mkdir()
        trap = shims / 'systemctl'
        trap.write_text('#!/bin/sh\nexit 91\n')
        trap.chmod(0o700)
        self.env['PATH'] = str(shims) + os.pathsep + os.environ['PATH']
        self.log = self.root / 'wallet.jsonl'
        # This public test credential is generated fixture input, never an owner login.
        auth = 'scratch-fixture:local-test-only'
        self.server = subprocess.Popen([str(MOCK), '--role', 'wallet', '--port', '0',
                                        '--fixtures', str(ROOT / 'tests/fixtures'), '--log', str(self.log),
                                        '--auth', auth], env=self.env, stdout=subprocess.DEVNULL,
                                       stderr=subprocess.PIPE, text=True)
        try:
            ready = select.select([self.server.stderr], [], [], 5)[0]
            self.assertTrue(ready, 'mock listener did not announce a port')
            line = self.server.stderr.readline()
            match = re.search(r'on 127\.0\.0\.1:(\d+)', line)
            self.assertIsNotNone(match, line)
            port = match.group(1)
            self.fm('settings', 'set', 'wallet.url', 'http://127.0.0.1:' + port)
            self.fm('settings', 'set', 'wallet.port', port)
            self.fm('settings', 'set', 'node.data_dir', str(self.root / 'chain'))
            self.node_log = self.root / 'node.log'
            self.fm('settings', 'set', 'node.log_file', str(self.node_log))
            run = self.root / 'state/run'
            run.mkdir(mode=0o700, exist_ok=True)
            login = run / ('monero-wallet-rpc.' + port + '.login')
            login.write_text(auth)
            login.chmod(0o600)
            self.fm('wallet', 'open', 'scratch', '--password-stdin', input='\n')
        except BaseException:
            self.server.terminate()
            self.server.wait(timeout=5)
            self.temp.cleanup()
            raise

    def tearDown(self):
        self.server.terminate()
        self.server.wait(timeout=5)
        self.server.stderr.close()
        self.temp.cleanup()

    def fm(self, *arguments, expected=0, input=None):
        result = subprocess.run([str(BINARY), *arguments, '--json'], env=self.env,
                                input=input, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def calls(self, method):
        return [row['body']['params'] for row in
                (json.loads(line) for line in self.log.read_text().splitlines())
                if row.get('method') == method and row.get('status') == 200]

    def test_legacy_mutations_refuse_pending_and_money_lock(self):
        commands = [('create_address', ('wallet', 'new-address', '--label', 'invoice')),
                    ('set_tx_notes', ('wallet', 'notes', 'set', TXID, 'reviewed')),
                    ('set_daemon', ('wallet', 'use-node', 'node.example:18081', '--yes'))]
        before = {method: len(self.calls(method)) for method, _ in commands}
        for name in ['pending.json', 'native-pending.json']:
            with self.subTest(name=name):
                record = self.root / 'state' / name
                # Invalid state must not disappear or allow a mutator to fall through.
                record.write_text('corrupt uncertainty')
                for method, command in commands:
                    result = subprocess.run([str(BINARY), *command, '--json'], env=self.env,
                                            capture_output=True, text=True, timeout=10)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertEqual(len(self.calls(method)), before[method])
                self.assertEqual(record.read_text(), 'corrupt uncertainty')
                record.unlink()
        with (self.root / 'state/send.lock').open('r+b') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for method, command in commands:
                self.fm(*command, expected=3)
                self.assertEqual(len(self.calls(method)), before[method])

    def test_new_address_and_note_use_exact_guarded_mutations(self):
        created = self.fm('wallet', 'new-address', '--label', 'invoice')
        fixture = json.loads((ROOT / 'tests/fixtures/wallet/create_address.json').read_text())
        self.assertEqual(created['selected'], fixture['address'])
        self.assertEqual(set(created), {'ok', 'schema', 'primary', 'addresses', 'selected'})
        call = self.calls('create_address')[-1]
        self.assertEqual(call['account_index'], 0)
        self.assertEqual(call['label'], 'invoice')
        self.fm('wallet', 'notes', 'set', TXID, 'reviewed')
        self.assertEqual(self.calls('set_tx_notes')[-1], {'txids': [TXID], 'notes': ['reviewed']})
        for method, command in [('create_address', ('wallet', 'new-address', '--label', 'x' * 1025)),
                                ('set_tx_notes', ('wallet', 'notes', 'set', TXID, 'bad\x1b[31m'))]:
            before = len(self.calls(method))
            self.fm(*command, expected=2)
            self.assertEqual(len(self.calls(method)), before)

    def test_one_off_url_auth_and_settings_are_preserved(self):
        config = self.root / 'config/config.toml'
        before = config.read_bytes()
        daemon_config = self.root / 'config/monerod.conf'
        daemon_config.write_text('rpc-login=scratch-node:fixture-only\n')
        daemon_config.chmod(0o600)
        for address in ['127.0.0.1:18081', 'http://127.0.0.1:18081', 'http://[::1]:18081']:
            result = self.fm('wallet', 'use-node', address, '--trusted', '--yes')
            self.assertFalse(result['persistent'])
            call = self.calls('set_daemon')[-1]
            self.assertEqual(call['username'], 'scratch-node')
            self.assertEqual(call['password'], 'fixture-only')
            self.assertEqual(config.read_bytes(), before)
        self.fm('wallet', 'use-node', 'node.example:18081', '--yes')
        self.assertNotIn('username', self.calls('set_daemon')[-1])
        self.assertNotIn('password', self.calls('set_daemon')[-1])
        self.assertEqual(config.read_bytes(), before)

    def test_node_log_sanitizes_before_filtering_and_refuses_fifo_and_symlink(self):
        self.node_log.write_text(
            '2026-10-14 12:00:00 INFO visible\x1b]52;c;CLIPBOARD_PAYLOAD\x07\n'
            '2026-10-14 12:00:01 INFO \x1bPDEVICE_PAYLOAD\x1b\\visible\n'
            '2026-10-14 12:00:02 password=synthetic-private-value\n'
            '2026-10-14 12:00:03 INFO \x1b]8;;https://private.example/\x1b\\visible\x1b]8;;\x1b\\\n'
            '2026-10-14 12:00:04 INFO \x1b]52;c;INCOMPLETE_PAYLOAD')
        before = self.fixture.read_bytes()
        result = self.fm('node', 'log')
        self.assertEqual(set(result), {'ok', 'schema', 'path', 'lines', 'truncated'})
        text = '\n'.join(result['lines'])
        self.assertIn('visible', text)
        for value in ['\x1b', 'CLIPBOARD_PAYLOAD', 'DEVICE_PAYLOAD', 'INCOMPLETE_PAYLOAD',
                      'synthetic-private-value', 'private.example']:
            self.assertNotIn(value, text)
            if value != '\x1b':
                self.assertEqual(self.fm('node', 'log', '--grep', value)['lines'], [])
        self.node_log.unlink()
        os.mkfifo(self.node_log, 0o600)
        self.fm('node', 'log', expected=3)
        self.node_log.unlink()
        target = self.root / 'regular.log'
        target.write_text('do not follow\n')
        self.node_log.symlink_to(target)
        self.fm('node', 'log', expected=3)
        self.assertEqual(self.fixture.read_bytes(), before)


if __name__ == '__main__':
    unittest.main(argv=[sys.argv[0], *remainder])
