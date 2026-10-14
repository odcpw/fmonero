#!/usr/bin/env python3
"""Bounded CLI measurements in a fully isolated state; never open an owner's wallet."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--runs', type=int, default=20)
args = parser.parse_args()
if not 5 <= args.runs <= 100:
    parser.error('--runs must be 5..100')
binary = args.binary.resolve()
address = '42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm'
with tempfile.TemporaryDirectory(prefix='fmonero-perf-') as directory:
    root = Path(directory)
    env = os.environ.copy()
    for key, name in {'FMONERO_STATE_DIR':'state', 'FMONERO_CONFIG_DIR':'config',
                      'FMONERO_CACHE_DIR':'cache', 'XDG_DATA_HOME':'data',
                      'FMONERO_SYSTEMD_USER_DIR':'systemd', 'XDG_CONFIG_HOME':'xdg-config'}.items():
        path = root / name
        path.mkdir(mode=0o700)
        env[key] = str(path)
    fixture = root / 'systemctl.txt'
    fixture.write_text('fmonero-wallet-rpc.service inactive disabled yes 0\nfmonero-monerod.service inactive disabled yes 0\n')
    env['FMONERO_SYSTEMCTL_FIXTURE'] = str(fixture)
    env['FMONERO_NODES_URL'] = 'http://127.0.0.1:1'
    env.pop('FMONERO_WALLET_PASSWORD', None)
    for key, value in [('node.url','http://127.0.0.1:1'),('wallet.url','http://127.0.0.1:2')]:
        subprocess.run([str(binary),'settings','set',key,value,'--json'], env=env,
                       check=True, stdout=subprocess.DEVNULL, timeout=5)
    def measure(command):
        with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as error:
            start = time.perf_counter()
            process = subprocess.Popen([str(binary), *command], env=env, stdout=output, stderr=error)
            deadline = start + 8
            while True:
                child, status, usage = os.wait4(process.pid, os.WNOHANG)
                if child:
                    process.returncode = os.waitstatus_to_exitcode(status)
                    break
                if time.perf_counter() > deadline:
                    process.kill()
                    child, status, usage = os.wait4(process.pid, 0)
                    process.returncode = os.waitstatus_to_exitcode(status)
                    raise RuntimeError('CLI benchmark exceeded its bounded deadline')
                time.sleep(.001)
            elapsed = (time.perf_counter() - start) * 1000
            output.seek(0); raw = output.read()
            error.seek(0); stderr = error.read().decode(errors='replace')
            if process.returncode:
                raise RuntimeError(f'benchmark command failed ({process.returncode}): {stderr}')
            return elapsed, usage.ru_maxrss, (usage.ru_utime + usage.ru_stime) * 1000, json.loads(raw)
    results = {}
    for name, command in [('app-status',['app','status','--json']),
                          ('closed-status',['status','--json']),
                          ('address-validation',['wallet','validate',address,'--json'])]:
        for _ in range(3): measure(command)
        samples = [measure(command) for _ in range(args.runs)]
        elapsed = sorted(s[0] for s in samples)
        results[name] = {'runs':args.runs, 'p50_ms':statistics.median(elapsed),
                         'sample_p95_ms':elapsed[math.ceil(.95*len(elapsed))-1],
                         'max_ms':max(elapsed), 'peak_child_rss_kib':max(s[1] for s in samples),
                         'median_child_cpu_ms':statistics.median(s[2] for s in samples)}
        document = samples[-1][3]
        if name == 'app-status':
            assert document['running'] is False and document['frontend_running'] is False
            results[name]['semantic_output'] = document
        elif name == 'closed-status':
            assert document['node']['reachable'] is False and document['wallet']['reachable'] is False
            results[name]['semantic_output'] = {'ok':document['ok'], 'node_reachable':document['node']['reachable'],
                                                'wallet_reachable':document['wallet']['reachable']}
        else:
            assert document['valid'] is True
            results[name]['semantic_output'] = {key:document.get(key) for key in ['valid','nettype','subaddress']}
    report = {'schema':1, 'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
              'clock':'perf_counter', 'scope':'isolated warm CLI, no owner wallet or service',
              'percentile_note':'p95 of the measured sample, not a production latency guarantee',
              'rss_note':'wait4 child high-water RSS includes process-spawn overhead; it is not a heap/allocation profile',
              'results':results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'output':str(args.output), 'results':results}, indent=2))
