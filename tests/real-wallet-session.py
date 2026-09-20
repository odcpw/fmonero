#!/usr/bin/env python3
"""Offline integration check using a disposable, empty real Monero wallet.

Build the release binary first. Never reads the owner's config or wallet files.
"""
import json, os, pathlib, socket, subprocess, tempfile, time
repo = pathlib.Path(__file__).resolve().parents[1]
binary = repo / 'target/release/fmonero'
with tempfile.TemporaryDirectory(prefix='fmonero-real-session-') as d:
    root = pathlib.Path(d)
    env = os.environ.copy()
    paths = {'FMONERO_STATE_DIR':'state','FMONERO_CONFIG_DIR':'config','FMONERO_CACHE_DIR':'cache', 'XDG_DATA_HOME':'data','FMONERO_SYSTEMD_USER_DIR':'systemd'}
    for key, folder in paths.items():
        p = root / folder
        p.mkdir(mode=0o700)
        env[key] = str(p)
    fixture = root / 'systemctl.txt'
    fixture.write_text('fmonero-wallet-rpc.service inactive disabled yes 0\nfmonero-monerod.service inactive disabled no -\n')
    env['FMONERO_SYSTEMCTL_FIXTURE'] = str(fixture)
    env['FMONERO_NODES_URL'] = 'http://127.0.0.1:1'
    wallets = root / 'wallets'
    wallets.mkdir(mode=0o700)
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
    def fm(*args):
        r = subprocess.run([str(binary), *args, '--json'], env=env, capture_output=True, text=True, timeout=20)
        if r.returncode: raise RuntimeError('fmonero operation failed: ' + r.stdout + r.stderr)
        return json.loads(r.stdout)
    fm('settings','set','wallet.url',f'http://127.0.0.1:{port}')
    fm('settings','set','wallet.port',str(port))
    fm('settings','set','wallet.dir',str(wallets))
    env['FMONERO_WALLET_PASSWORD'] = ''
    run_dir = root / 'state' / 'run'
    run_dir.mkdir(mode=0o700, exist_ok=True)
    login_file = run_dir / f'monero-wallet-rpc.{port}.login'
    proc = None
    command = None
    def cold_start(*args):
        global proc, command
        fixture.write_text('fmonero-wallet-rpc.service inactive disabled yes 0\nfmonero-monerod.service inactive disabled no -\n')
        command = subprocess.Popen([str(binary), *args, '--json'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, stdin=subprocess.PIPE, text=True)
        if "--password-stdin" in args:
            command.stdin.write("\n")
            command.stdin.flush()
        # The fixture changes only after fmonero constructed its client and tried its first
        # probe. Start real Monero then, so its generated login cannot exist in that client.
        deadline = time.monotonic() + 10
        while fixture.read_text().split()[1] != 'active':
            if command.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError('fmonero did not request the isolated service start')
            time.sleep(.02)
        proc = subprocess.Popen(['monero-wallet-rpc', '--offline', '--rpc-bind-ip','127.0.0.1','--rpc-bind-port',str(port),'--wallet-dir',str(wallets),'--log-file',str(root/'wallet.log'),'--log-level','0','--non-interactive'], cwd=run_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        out, err = command.communicate(timeout=40)
        if command.returncode:
            raise RuntimeError('authenticated cold start failed: ' + out + err)
        result = json.loads(out)
        assert result['unit_started'] is True
    try:
        cold_start('wallet', 'create', 'regression', '--allow-empty-password', '--yes')
        before = fm('status')['wallet']['primary_address']
        assert before
        assert fm('status')['wallet']['height'] is not None
        for address in ('127.0.0.1:18081', '127.0.0.1:28081'):
            result = fm('node','remote',address,'--yes')
            assert result['wallet_unit_restarted'] is False
            assert result['wallet_must_reopen'] is None
            assert proc.poll() is None
            assert fm('status')['wallet']['primary_address'] == before
            assert fm('status')['wallet']['height'] is not None
        fm('wallet', 'close')
        old_login = login_file.read_bytes()
        proc.terminate()
        proc.wait(timeout=10)
        cold_start('wallet', 'open', 'regression', '--password-stdin')
        assert login_file.read_bytes() != old_login
        assert fm('status')['wallet']['primary_address'] == before
        fm('wallet', 'close')
        result = fm('node','remote','127.0.0.1:38081','--yes')
        assert result['wallet_unit_restarted'] is False
        print('PASS: authenticated cold-start creation, reopening after login rotation, two node changes without closing, and closed-wallet selection. Offline, empty test wallet; no transaction built or sent.')
    finally:
        for child in (command, proc):
            if child is not None and child.poll() is None:
                child.terminate()
                try: child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill(); child.wait()
