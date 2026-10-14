#!/usr/bin/env python3
"""Root-only, actual Monero TLS handshake qualification; no blockchain simulation.

python3 tests/native-tls.py --binary target/debug/fmonero --output /tmp/native-tls.json
Uses only a fresh scratch wallet and loopback gateway. Never submits a transaction.
The gateway records counters only. A bootstrap-only zero-height reply permits
wallet generation; later requests receive protocol errors, never chain data.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import socket
import ssl
import stat
import subprocess
import tempfile
import threading
import time


class Refusal(Exception):
    """Only fixed, public diagnostic labels may enter this exception."""


def require(condition, label):
    if not condition:
        raise Refusal(label)


def counter_delta(baseline, observed):
    return {name: observed[name] - value for name, value in baseline.items()}


def stop(process):
    if process is not None and process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


def bounded(argv, *, env=None, cwd=None, data=b"", timeout=30):
    require(len(data) <= 4096, "CLI input exceeded its bound")
    process = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, umask=0o077)
    buffers = [bytearray(), bytearray()]
    try:
        # At most PIPE_BUF bytes on a new empty pipe; never wait for a reader.
        try:
            process.stdin.write(data)
        except BrokenPipeError:
            pass
        process.stdin.close()
        end = time.monotonic() + timeout
        with selectors.DefaultSelector() as selector:
            for index, pipe in enumerate((process.stdout, process.stderr)):
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ, index)
            while selector.get_map():
                require(time.monotonic() < end, "subprocess exceeded its deadline")
                for key, _ in selector.select(min(0.1, max(0, end - time.monotonic()))):
                    chunk = os.read(key.fd, 8192)
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        require(sum(map(len, buffers)) + len(chunk) <= 262144,
                                "subprocess output exceeded its bound")
                        buffers[key.data].extend(chunk)
            process.wait(timeout=max(0.01, end - time.monotonic()))
        if data:
            require(all(data[:-1] not in buffer for buffer in buffers),
                    "protected CLI input appeared in output")
        return process.returncode, bytes(buffers[0])
    finally:
        stop(process)
        for pipe in (process.stdin, process.stdout, process.stderr):
            pipe.close()
        for buffer in buffers:
            buffer[:] = b"\0" * len(buffer)


def owner_services():
    code, output = bounded(["systemctl", "--user", "show", "--no-pager",
        "-p", "Id", "-p", "ActiveState", "-p", "SubState", "-p", "MainPID",
        "fmonero-monerod.service", "fmonero-wallet-rpc.service"], timeout=5)
    require(code == 0 and output.count(b"MainPID=") == 2,
            "could not snapshot owner service state")
    return output


class Gateway:
    def __init__(self, certificate, key):
        self.context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.context.minimum_version = ssl.TLSVersion.TLSv1_2
        self.context.load_cert_chain(certificate, key)
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(8)
        self.listener.settimeout(0.2)
        self.port = self.listener.getsockname()[1]
        self.lock, self.done = threading.Lock(), threading.Event()
        self.current = None
        self.bootstrap = True
        self.counts = dict(tls_preambles=0, plaintext_preambles=0, http_requests=0,
                           handshake_failures=0, server_errors=0)
        self.worker = threading.Thread(target=self.serve, daemon=True)
        self.worker.start()

    def count(self, name):
        with self.lock:
            self.counts[name] += 1

    def snapshot(self):
        with self.lock:
            return dict(self.counts)

    def serve(self):
        while not self.done.is_set():
            try:
                raw, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                if not self.done.is_set():
                    self.count("server_errors")
                return
            connection = raw
            try:
                with self.lock:
                    self.current = raw
                raw.settimeout(2)
                end = time.monotonic() + 2
                while True:
                    prefix = raw.recv(3, socket.MSG_PEEK)
                    if len(prefix) >= 3:
                        break
                    require(prefix and time.monotonic() < end, "incomplete gateway preamble")
                    time.sleep(0.01)
                if prefix[:2] != b"\x16\x03":
                    self.count("plaintext_preambles")
                    continue
                self.count("tls_preambles")
                require(self.snapshot()["tls_preambles"] <= 256, "gateway connection bound")
                connection = self.context.wrap_socket(raw, server_side=True,
                                                      do_handshake_on_connect=False)
                with self.lock:
                    self.current = connection
                try:
                    connection.do_handshake()
                except ssl.SSLError:
                    self.count("handshake_failures")
                    continue
                self.http(connection)
            except (OSError, Refusal, ValueError):
                if not self.done.is_set():
                    self.count("server_errors")
            finally:
                connection.close()
                raw.close()
                with self.lock:
                    self.current = None

    def http(self, connection):
        end, header = time.monotonic() + 3, bytearray()
        while not header.endswith(b"\r\n\r\n"):
            require(len(header) < 8192 and time.monotonic() < end, "HTTP header bound")
            connection.settimeout(max(0.01, end - time.monotonic()))
            byte = connection.recv(1)
            require(byte, "incomplete HTTP header")
            header.extend(byte)
        lines = bytes(header).split(b"\r\n")
        require(lines[0].startswith((b"POST /", b"GET /")), "unexpected HTTP request")
        lengths = [line.split(b":", 1)[1].strip() for line in lines[1:]
                   if line.lower().startswith(b"content-length:")]
        require(len(lengths) <= 1 and not any(line.lower().startswith(b"transfer-encoding:")
                for line in lines[1:]), "unsupported HTTP framing")
        remaining = int(lengths[0]) if lengths else 0
        require(0 <= remaining <= 262144, "HTTP body bound")
        while remaining:
            require(time.monotonic() < end, "HTTP body deadline")
            connection.settimeout(max(0.01, end - time.monotonic()))
            # Consume/discard: no body, method, address or transaction is recorded.
            received = len(connection.recv(min(8192, remaining)))
            require(received, "incomplete HTTP body")
            remaining -= received
        self.count("http_requests")
        payload = b'{"jsonrpc":"2.0","id":"0","error":{"code":-32601,"message":"TLS qualifier has no blockchain"},"status":"FAILED"}'
        with self.lock:
            bootstrap = self.bootstrap
        if bootstrap and lines[0].split(b" ", 2)[1] == b"/getheight":
            # Monero asks before generating a wallet. This synthetic bootstrap
            # response is not a chain observation and is disabled after creation.
            payload = b'{"height":0,"status":"OK","untrusted":true}'
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
            + f"Content-Length: {len(payload)}\r\nConnection: close\r\n\r\n".encode() + payload)

    def close(self):
        self.done.set()
        self.listener.close()
        with self.lock:
            if self.current is not None:
                try:
                    self.current.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
        self.worker.join(timeout=5)
        require(not self.worker.is_alive(), "gateway cleanup did not finish")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    require(binary.is_file() and os.access(binary, os.X_OK), "binary is not an executable file")
    require(not args.output.exists() and not args.output.is_symlink(), "report already exists")
    engine, openssl = shutil.which("monero-wallet-rpc"), shutil.which("openssl")
    require(engine and openssl, "Monero and openssl are required")
    ca = next((Path(p) for p in ("/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/cert.pem")
               if Path(p).is_file()), None)
    require(ca is not None, "system CA bundle is required")
    before, started = owner_services(), time.monotonic()
    report, process, gateway = None, None, None
    try:
        with tempfile.TemporaryDirectory(prefix="fmonero-native-tls-") as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            env = {"PATH": os.defpath, "HOME": str(Path.home()), "LANG": "C", "LC_ALL": "C"}
            for name, child in {"FMONERO_STATE_DIR": "state", "FMONERO_CONFIG_DIR": "config",
                    "FMONERO_CACHE_DIR": "cache", "XDG_DATA_HOME": "data",
                    "FMONERO_SYSTEMD_USER_DIR": "units", "TMPDIR": "tmp"}.items():
                (root / child).mkdir(mode=0o700)
                env[name] = str(root / child)
            run, wallets = root / "state/run", root / "wallets"
            run.mkdir(mode=0o700)
            wallets.mkdir(mode=0o700)
            fixture = root / "systemctl.txt"
            env["FMONERO_SYSTEMCTL_FIXTURE"] = str(fixture)
            env["FMONERO_NODES_URL"] = "http://127.0.0.1:1/no-discovery"
            certificate, key = root / "localhost.pem", root / "localhost.key"
            code, _ = bounded([openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                "-sha256", "-days", "1", "-subj", "/CN=localhost",
                "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
                "-addext", "basicConstraints=critical,CA:TRUE",
                "-keyout", str(key), "-out", str(certificate)], env=env, cwd=root)
            require(code == 0, "scratch certificate generation failed")
            try:
                gateway = Gateway(certificate, key)
                endpoint = f"https://127.0.0.1:{gateway.port}"
                with socket.socket() as reservation:
                    reservation.bind(("127.0.0.1", 0))
                    port = reservation.getsockname()[1]
                process = subprocess.Popen([engine, "--stagenet", "--no-initial-sync",
                    "--non-interactive", "--wallet-dir", str(wallets), "--rpc-bind-ip", "127.0.0.1",
                    "--rpc-bind-port", str(port), "--rpc-ssl", "disabled", "--untrusted-daemon",
                    "--daemon-address", endpoint, "--daemon-ssl", "enabled",
                    "--daemon-ssl-ca-certificates", str(certificate), "--shared-ringdb-dir", str(root / "ringdb"),
                    "--log-file", str(root / "wallet.log"), "--log-level", "0",
                    "--max-log-file-size", "1048576", "--max-log-files", "1", "--max-concurrency", "2"],
                    cwd=run, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, start_new_session=True, umask=0o077)
                fixture.write_text(f"fmonero-wallet-rpc.service active disabled yes {process.pid}\n"
                                   "fmonero-monerod.service inactive disabled no -\n")
                fixture.chmod(0o600)

                def cli(words, data=b"", codes=(0,)):
                    require(process.poll() is None, "scratch wallet engine exited")
                    remaining = 240 - (time.monotonic() - started)
                    require(remaining > 0, "qualification exceeded its total deadline")
                    code, output = bounded([str(binary), *words, "--json"], env=env, cwd=root,
                                           data=data, timeout=min(40, remaining))
                    reason = "invalid_json"
                    try:
                        doc = json.loads(output)
                    except ValueError:
                        doc = None
                    else:
                        reason = "invalid_envelope"
                        if isinstance(doc, dict):
                            candidate = doc.get("reason")
                            if candidate is None:
                                reason = "unspecified"
                            elif isinstance(candidate, str) and candidate in {
                                "bad_args", "unreachable", "rpc_error", "bad_password", "not_open",
                                "wallet_peer_unverified", "transport_policy", "killed_at_deadline",
                                "needs_yes", "wallet_changed", "remote_address_rejected",
                            }:
                                reason = candidate
                            else:
                                reason = "unrecognized_reason"
                    # Exit status and fixed/allowlisted labels only: never emit
                    # captured stdout/stderr or a backend's arbitrary error text.
                    detail = f"{' '.join(words[:2])}: exit {code}, reason {reason}"
                    require(code in codes, "unexpected scratch CLI exit for " + detail)
                    require(isinstance(doc, dict) and doc.get("ok") is (code == 0),
                            "CLI JSON envelope mismatch for " + detail)
                    return code, doc

                credential = run / f"monero-wallet-rpc.{port}.login"
                deadline = time.monotonic() + 10
                while True:
                    require(process.poll() is None and time.monotonic() < deadline,
                            "scratch wallet listener did not start")
                    if credential.exists():
                        metadata = credential.lstat()
                        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid()
                                and metadata.st_mode & 0o077 == 0, "scratch Digest file is not private")
                        try:
                            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                                break
                        except OSError:
                            pass
                    time.sleep(0.05)
                # Only core reads Monero's generated private Digest login. No Python RPC client.
                for name, value in [("node.network", "stagenet"), ("wallet.url", f"http://127.0.0.1:{port}"),
                        ("wallet.port", str(port)), ("wallet.dir", str(wallets)), ("node.remote", endpoint),
                        ("node.mode", "remote"), ("node.tls", "required"), ("node.ca_file", str(certificate)),
                        ("timeouts.refresh_sec", "15"), ("timeouts.wallet_sec", "10")]:
                    cli(["settings", "set", name, value])
                bootstrap_before = gateway.snapshot()
                password = bytearray(os.urandom(32).hex(), "ascii") + b"\n"
                try:
                    cli(["wallet", "create", "tls-scratch", "--password-stdin", "--yes"], data=password)
                finally:
                    password[:] = b"\0" * len(password)
                with gateway.lock:
                    gateway.bootstrap = False
                bootstrap = gateway.snapshot()
                bootstrap_delta = counter_delta(bootstrap_before, bootstrap)
                require(bootstrap_delta["tls_preambles"] > 0 and bootstrap_delta["http_requests"] > 0
                        and bootstrap["plaintext_preambles"] == 0 and bootstrap["server_errors"] == 0,
                        "scratch wallet bootstrap did not use trusted TLS")
                # Creation must succeed first. Reconfigure this same real wallet
                # to distrust the certificate; bootstrap HTTP is not refusal evidence.
                cli(["settings", "set", "node.ca_file", str(ca)])
                cli(["wallet", "use-node", endpoint, "--yes"])
                untrusted_before = gateway.snapshot()
                refused, _ = cli(["wallet", "refresh"], codes=(4, 5))
                untrusted = gateway.snapshot()
                untrusted_delta = counter_delta(untrusted_before, untrusted)
                require(untrusted_delta["tls_preambles"] > 0 and untrusted_delta["http_requests"] == 0
                        and untrusted_delta["plaintext_preambles"] == 0 and untrusted["plaintext_preambles"] == 0
                        and untrusted["server_errors"] == 0, "untrusted TLS was not cleanly refused")
                cli(["settings", "set", "node.ca_file", str(certificate)])
                cli(["wallet", "use-node", endpoint, "--yes"])
                trusted_before = gateway.snapshot()
                accepted, _ = cli(["wallet", "refresh"], codes=(0, 4, 5))
                trusted = gateway.snapshot()
                trusted_delta = counter_delta(trusted_before, trusted)
                require(trusted_delta["tls_preambles"] > 0 and trusted_delta["http_requests"] > 0
                        and trusted_delta["plaintext_preambles"] == 0
                        and trusted["plaintext_preambles"] == 0 and trusted["server_errors"] == 0,
                        "trusted certificate did not carry actual HTTP over TLS")
                cli(["wallet", "close"])
                report = dict(schema=1, scope="isolated Monero TLS handshake only", blockchain_qualified=False,
                    bootstrap=dict(baseline=bootstrap_before, counters=bootstrap, delta=bootstrap_delta,
                                   bootstrap_only_getheight_zero=True),
                    untrusted=dict(refresh_exit=refused, baseline=untrusted_before,
                                   counters=untrusted, delta=untrusted_delta),
                    trusted=dict(refresh_exit=accepted, protocol_outcome="refresh_success" if accepted == 0
                                 else "minimal_gateway_backend_failure", baseline=trusted_before,
                                 counters=trusted, delta=trusted_delta))
            finally:
                try:
                    stop(process)
                finally:
                    if gateway is not None:
                        gateway.close()
                require(gateway is None or (gateway.snapshot()["plaintext_preambles"] == 0
                        and gateway.snapshot()["server_errors"] == 0), "gateway failed before cleanup")
                if report is not None:
                    report["final_counters"] = gateway.snapshot()
    finally:
        require(owner_services() == before, "owner service state changed after scratch cleanup")
    require(report is not None, "qualification produced no report")
    with binary.open("rb") as source:
        report["binary_sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
    report.update(cleanup_complete=True, owner_service_state_unchanged=True,
                  elapsed_sec=round(time.monotonic() - started, 3))
    # Publish only after process, socket/thread and temporary-directory cleanup.
    with tempfile.NamedTemporaryFile(dir=args.output.parent, delete=False) as output:
        name = output.name
        try:
            output.write((json.dumps(report, indent=2) + "\n").encode())
            output.flush()
            os.fsync(output.fileno())
            os.link(name, args.output)  # atomic, refuses existing reports/symlinks
        finally:
            os.unlink(name)
    print("PASS: native untrusted/trusted TLS handshake checks; no blockchain qualification")


def cancelled(*_):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    raise KeyboardInterrupt


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, cancelled)
    signal.signal(signal.SIGINT, cancelled)
    try:
        main()
    except (Refusal, OSError, ValueError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        # Never print raw process/protocol output, certificate/key material or secrets.
        print("FAIL:", str(error) if isinstance(error, Refusal) else type(error).__name__)
        raise SystemExit(1)
