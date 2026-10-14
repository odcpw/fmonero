#!/usr/bin/env python3
"""Root-run simulated hardware protocol checks; ZERO device/crypto qualification.

python3 tests/hardware-protocol.py --binary target/release/fmonero \
    --mock target/debug/fmonero-mock --output /tmp/hardware-protocol.json
Uses an authenticated loopback mock and a PATH-only Python native-engine shim.
The shim never invokes Monero or USB code. Linux x86_64 seccomp additionally
blocks new file opens, networking, ioctls and exec after scratch descriptors
are prepared. No actual wallet, hardware, cryptography or broadcast is tested.
"""
import argparse
import ctypes
import errno
import fcntl
import json
import hashlib
import os
from pathlib import Path
import platform
import re
import resource
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
# Public test inputs only. The password travels to fmonero solely over stdin.
PASSWORD = b"public-hardware-protocol-fixture"
AUTH = "hardware-fixture:public-rpc-fixture"
LIMIT = 1024 * 1024


class Refusal(Exception):
    """Fixed public diagnostics only; never raw child/protocol text."""


def require(condition, label):
    if not condition:
        raise Refusal(label)


def private_json(path, value):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        json.dump(value, output)


def deny_device_access():
    """Fail closed outside this syscall ABI; no optional sandbox fallback."""
    require(platform.machine() == "x86_64", "shim requires Linux x86_64 seccomp")
    class Filter(ctypes.Structure):
        _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte),
                    ("jf", ctypes.c_ubyte), ("k", ctypes.c_uint)]
    class Program(ctypes.Structure):
        _fields_ = [("length", ctypes.c_ushort), ("filters", ctypes.POINTER(Filter))]
    # Validate arch, then reject x32 numbers and every device/network/exec entry
    # used by this boundary. Only inherited fd3 and owned scratch/output fds remain.
    rules = [(0x20, 0, 0, 4), (0x15, 1, 0, 0xc000003e), (0x06, 0, 0, 0x80000000),
             (0x20, 0, 0, 0), (0x35, 0, 1, 0x40000000), (0x06, 0, 0, 0x80000000)]
    # open, ioctl, socket, connect, execve, creat, ptrace, mount, openat,
    # open_by_handle_at, execveat, io_uring_setup, openat2.
    for number in [2, 16, 41, 42, 59, 85, 101, 165, 257, 304, 322, 425, 437]:
        rules += [(0x15, 0, 1, number), (0x06, 0, 0, 0x50000 | errno.EPERM)]
    rules.append((0x06, 0, 0, 0x7fff0000))
    filters = (Filter * len(rules))(*(Filter(*rule) for rule in rules))
    libc = ctypes.CDLL(None, use_errno=True)
    require(libc.prctl(38, 1, 0, 0, 0) == 0, "shim no-new-privileges failed")
    program = Program(len(rules), filters)
    require(libc.prctl(22, 2, ctypes.byref(program), 0, 0) == 0, "shim seccomp failed")
    try:
        os.open("/dev/null", os.O_RDONLY)  # harmless proof that device opens are denied
    except OSError as error:
        require(error.errno == errno.EPERM, "shim device-open denial failed")
    else:
        raise Refusal("shim device-open denial failed")


def shim():
    signal.alarm(8)  # independent orphan bound, even if the CLI supervisor dies
    root = Path(os.environ["FMONERO_HW_TEST_ROOT"])
    case = json.loads((root / "case.json").read_bytes())
    argv = sys.argv[1:]
    require(PASSWORD.decode() not in "\0".join(argv), "password appeared in native argv")
    require(all(PASSWORD.decode() not in value for value in os.environ.values()),
            "password appeared in native environment")
    require("FMONERO_WALLET_PASSWORD" not in os.environ, "native password environment present")
    stage = Path.cwd().resolve()
    require(stage.parent == root / "state/run" and stat.S_IMODE(stage.stat().st_mode) == 0o700,
            "native working directory is not private scratch")
    ring = stage / "ringdb"
    ring.mkdir(mode=0o700)
    target = root / "wallets" / case["name"]
    require(argv == case["expected_argv"], "native argument contract changed")
    info = os.fstat(3)
    seals = fcntl.fcntl(3, fcntl.F_GET_SEALS)
    required = fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL
    require(stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
            and info.st_size == len(PASSWORD) and seals & required == required,
            "native protected fd3 metadata or seals differ")
    trace = os.open(root / "native.jsonl", os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    files = [os.open(str(target) + suffix, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
             for suffix in ["", ".keys"]]
    deny_device_access()
    # No fd3 reads, password/body/hash logging, wallet bytes or crypto simulation.
    event = dict(case=case["name"], argv=argv, pid=os.getpid(), fd3_present=True,
                 fd3_size=info.st_size, fd3_sealed=True, private_ringdb=True,
                 secret_argv_env_absent=True, device_access_denied=True,
                 fixture_address=case["address"])
    os.write(trace, (json.dumps(event) + "\n").encode())
    os.close(trace)
    for fd in files:
        os.write(fd, b"SIMULATED PROTOCOL MARKER; NOT A MONERO WALLET\n")
        os.close(fd)
    if case["mode"] == "timeout":
        time.sleep(20)  # core --timeout 2 must terminate this after possible creation
    if case["mode"] != "missing_address":
        print(case["address"], flush=True)
    return 19 if case["mode"] == "nonzero" else 0


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
            process.wait(timeout=3)


def exited(pid):
    try:
        os.kill(pid, 0)  # liveness only; never signal an unowned/reused PID
    except ProcessLookupError:
        return True
    return False


def bounded(argv, env, data=b""):
    require(len(data) <= 4096, "CLI input limit")
    child = subprocess.Popen(argv, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, start_new_session=True, umask=0o077)
    buffers = [bytearray(), bytearray()]
    try:
        try:
            child.stdin.write(data)
        except BrokenPipeError:
            pass
        child.stdin.close()
        end = time.monotonic() + 15
        with selectors.DefaultSelector() as selector:
            for index, pipe in enumerate([child.stdout, child.stderr]):
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ, index)
            while selector.get_map():
                require(time.monotonic() < end, "CLI deadline exceeded")
                for key, _ in selector.select(0.1):
                    chunk = os.read(key.fd, 8192)
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        require(sum(map(len, buffers)) + len(chunk) <= 131072, "CLI output limit")
                        buffers[key.data].extend(chunk)
            child.wait(timeout=max(0.01, end - time.monotonic()))
        require(all(PASSWORD not in value for value in buffers), "password appeared in CLI output")
        return child.returncode, json.loads(buffers[0])
    finally:
        stop(child)
        for pipe in [child.stdin, child.stdout, child.stderr]:
            pipe.close()
        for value in buffers:
            value[:] = b"\0" * len(value)


def public_events(root):
    path = root / "native.jsonl"
    if not path.exists():
        return []
    require(path.stat().st_size <= 65536, "native trace limit")
    return [json.loads(line) for line in path.read_bytes().splitlines()]


def children_exited(root):
    end = time.monotonic() + 10
    for event in public_events(root):
        while not exited(event["pid"]):
            require(time.monotonic() < end, "native shim did not exit during cleanup")
            time.sleep(0.05)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["binary", "mock", "output"]:
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    binary, mock = args.binary.resolve(strict=True), args.mock.resolve(strict=True)
    require(not args.output.exists() and not args.output.is_symlink(), "report already exists")
    checks, traces = [], []
    with tempfile.TemporaryDirectory(prefix="fmonero-hardware-protocol-") as temporary:
        root = Path(temporary)
        env = {"HOME": str(root), "LC_ALL": "C", "LANG": "C", "FMONERO_HW_TEST_ROOT": str(root)}
        for key, name in {"FMONERO_STATE_DIR": "state", "FMONERO_CONFIG_DIR": "config",
                          "FMONERO_CACHE_DIR": "cache", "XDG_DATA_HOME": "data",
                          "FMONERO_SYSTEMD_USER_DIR": "units"}.items():
            (root / name).mkdir(mode=0o700)
            env[key] = str(root / name)
        (root / "state/run").mkdir(mode=0o700)
        (root / "wallets").mkdir(mode=0o700)
        (root / "bin").mkdir(mode=0o700)
        fixture = root / "systemctl.txt"
        fixture.write_text("fmonero-wallet-rpc.service active disabled yes -\n"
                           "fmonero-monerod.service inactive disabled no -\n")
        env.update(FMONERO_SYSTEMCTL_FIXTURE=str(fixture), FMONERO_NODES_URL="http://127.0.0.1:1",
                   PATH=str(root / "bin"))  # no real engine/systemctl fallback exists
        shim_path = root / "bin/monero-wallet-cli"
        shim_path.write_text("#!" + sys.executable + "\n" + Path(__file__).read_text().split("\n", 1)[1])
        shim_path.chmod(0o700)
        for name in ["systemctl", "monerod", "monero-wallet-rpc"]:
            trap = root / "bin" / name
            trap.write_text("#!/bin/sh\n: > '" + str(root / "forbidden-command") + "'\nexit 93\n")
            trap.chmod(0o700)
        fixtures = root / "fixtures/wallet"
        fixtures.mkdir(parents=True, mode=0o700)
        for name in ["get_version", "close_wallet", "open_wallet", "auto_refresh", "set_daemon"]:
            (fixtures / (name + ".json")).write_bytes((ROOT / "tests/fixtures/wallet" / (name + ".json")).read_bytes())
        private_json(fixtures / "set_attribute.json", {})
        addresses = {"mainnet": json.loads((ROOT / "tests/fixtures/wallet/get_address.json").read_bytes())["address"],
                     "stagenet": json.loads((ROOT / "tests/fixtures/wallet/validate_address-wrong-nettype.json").read_bytes())["_request_address"]}
        mismatch = json.loads((ROOT / "tests/fixtures/wallet/validate_address-subaddress.json").read_bytes())["_request_address"]
        # Request bodies (including the public fixture password) never enter a disk log.
        log = os.memfd_create("hardware-protocol-rpc", os.MFD_CLOEXEC)
        server = None
        try:
            def limit_mock():
                resource.setrlimit(resource.RLIMIT_FSIZE, (LIMIT, LIMIT))
            server = subprocess.Popen([str(mock), "--role", "wallet", "--port", "0", "--fixtures",
                str(root / "fixtures"), "--auth", AUTH, "--log", f"/proc/self/fd/{log}"],
                env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                pass_fds=(log,), start_new_session=True, umask=0o077, preexec_fn=limit_mock)
            announcement = bytearray()
            end = time.monotonic() + 5
            os.set_blocking(server.stderr.fileno(), False)
            while b"\n" not in announcement:
                require(time.monotonic() < end and server.poll() is None, "mock startup deadline")
                try:
                    announcement.extend(os.read(server.stderr.fileno(), 256))
                except BlockingIOError:
                    time.sleep(0.01)
                require(len(announcement) <= 4096, "mock startup output limit")
            match = re.search(rb"on 127\.0\.0\.1:(\d+)\n", announcement)
            require(match is not None, "mock did not announce loopback listener")
            port = match[1].decode()

            def fm(*words, expected=0, data=b""):
                code, reply = bounded([str(binary), "--json", "--timeout", "2", *words], env, data)
                reason = reply.get("reason")
                safe = reason if reason in {"wallet_exists", "pending_send", "send_in_progress", "auth_refused"} else "unclassified"
                require(code == expected, f"CLI exit {code}, expected {expected}, reason {safe}")
                return reply

            for key, value in [("wallet.url", "http://127.0.0.1:" + port), ("wallet.port", port),
                               ("wallet.dir", str(root / "wallets")), ("node.url", "http://127.0.0.1:1")]:
                fm("settings", "set", key, value)
            login = root / "state/run" / ("monero-wallet-rpc." + port + ".login")
            login.write_text(AUTH)
            login.chmod(0o600)

            def attempt(name, device="ledger", network="mainnet", mode="success", expected=0, reason=None):
                fm("settings", "set", "node.network", network)
                address = addresses[network]
                (fixtures / "get_address.json").write_text(json.dumps({"address": address,
                    "addresses": [{"address": address, "address_index": 0, "label": "fixture", "used": False}]}))
                native = ["--log-file", "/dev/null", "--log-level", "0", "--max-concurrency", "2"]
                if network != "mainnet": native += ["--" + network]
                native += ["--offline", "--generate-from-device", str(root / "wallets" / name),
                           "--hw-device", device.capitalize(), "--restore-height", "123",
                           "--password-file", "/proc/self/fd/3", "--shared-ringdb-dir", "ringdb", "--command", "address"]
                case = root / "case.json"
                case.unlink(missing_ok=True)
                private_json(case, dict(name=name, mode=mode, address=mismatch if mode == "mismatch" else address, expected_argv=native))
                before, offset = len(public_events(root)), os.fstat(log).st_size
                started = time.monotonic()
                try:
                    reply = fm("wallet", "tools", "care", "hardware", name, device,
                               "--restore-height", "123", "--secret-stdin", "--yes", expected=expected, data=PASSWORD + b"\n")
                except Refusal as error:
                    raise Refusal(f"case {name}: {error}") from None
                elapsed = time.monotonic() - started
                require(os.fstat(log).st_size <= LIMIT, "RPC memory-log limit")
                rows = [json.loads(line) for line in os.pread(log, LIMIT, offset).splitlines()]
                calls = [row for row in rows if row.get("status") == 200]
                events = public_events(root)[before:]
                if reason:
                    require(reply.get("reason") == reason and not events and not rows, "pre-child interlock failed")
                else:
                    require(len(events) == 1 and events[0]["argv"] == native, "native dispatch count/argv differs")
                    require(events[0]["fd3_sealed"] and events[0]["device_access_denied"], "native protection missing")
                    require(exited(events[0]["pid"]), "CLI returned with native child still alive")
                    if mode == "timeout":
                        require(elapsed < 5, "native timeout relied on the shim fallback alarm")
                    opened = [row for row in calls if row["method"] == "open_wallet"]
                    state = "ready" if mode == "success" else "needs_setup" if mode == "mismatch" else "unknown"
                    require(reply.get("state") == state, "hardware outcome classification differs")
                    require(reply.get("restore_height") == 123, "restore height outcome differs")
                    require(len(opened) == (1 if mode in {"success", "mismatch"} else 0), "unexpected reopen count")
                    if opened:
                        require(opened[0]["body"]["params"] == {"filename": name, "password": PASSWORD.decode(), "autosave_current": True}, "reopen inputs differ")
                        methods = [row["method"] for row in calls]
                        require(methods.index("close_wallet") < methods.index("open_wallet") < methods.index("get_address"), "reopen identity order differs")
                    if mode == "success":
                        require(reply.get("address") == address and reply.get("network") == network, "ready identity differs")
                        require(any(row["method"] == "set_attribute" and row["body"]["params"].get("value") == device for row in calls), "device provenance not recorded")
                    else:
                        require(reply.get("warning") and not any(row["method"] == "set_attribute" for row in calls), "uncertainty lost")
                    session = json.loads((root / "state/state.json").read_bytes())
                    require(session.get("last_open") == ([name, network] if mode == "success" else None), "session identity differs from outcome")
                    require((root / "wallets" / (name + ".keys")).read_bytes().startswith(b"SIMULATED"), "possible scratch artifact lost")
                outcome = "guard_refused" if reason else "simulated_protocol_success" if mode == "success" else reply["state"]
                checks.append(dict(case=name, outcome=outcome, core_state=reply.get("state"), exit=expected,
                                   evidence="simulated_protocol_only"))

            for device in ["ledger", "trezor"]:
                for network in addresses:
                    attempt(device + "-" + network, device, network)
            for mode in ["nonzero", "missing_address", "mismatch", "timeout"]:
                attempt(mode, mode=mode, expected=7)
            for suffix in ["", ".keys", ".address.txt"]:
                existing = root / "wallets" / ("existing" + suffix)
                existing.write_bytes(b"public preexisting marker")
                attempt("existing", expected=3, reason="wallet_exists")
                checks[-1]["existing_suffix"] = suffix
                require(existing.read_bytes() == b"public preexisting marker", "existing file changed")
                existing.unlink()
            for filename in ["pending.json", "native-pending.json"]:
                pending = root / "state" / filename
                pending.write_bytes(b"unresolved fixture")
                attempt("guarded-" + filename, expected=3, reason="pending_send")
                require(pending.read_bytes() == b"unresolved fixture", "pending record changed")
                pending.unlink()
            with (root / "state/send.lock").open("r+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                attempt("locked", expected=3, reason="send_in_progress")
            require(not (root / "forbidden-command").exists(), "unexpected engine or service command")
            traces = public_events(root)
        finally:
            try:
                stop(server)
                if server is not None: server.stderr.close()
            finally:
                os.close(log)
                children_exited(root)
    require(not root.exists(), "scratch cleanup incomplete")
    with args.binary.open("rb") as source:
        binary_sha = hashlib.file_digest(source, "sha256").hexdigest()
    report = dict(ok=True, binary_sha256=binary_sha, qualification="simulated_software_protocol_only", checks=checks, native=traces,
                  cleanup_complete=True, physical_hardware_qualified=False, cryptography_qualified=False,
                  real_monero_device_qualified=False, funded_payment_qualified=False,
                  mock_pid_fixture="unknown", rpc_bodies_persisted=False)
    # Publish only after all owned processes and scratch artifacts are gone.
    with tempfile.NamedTemporaryFile(dir=args.output.parent, delete=False) as output:
        name = output.name
        try:
            output.write((json.dumps(report, indent=2) + "\n").encode())
            output.flush()
            os.fsync(output.fileno())
            os.link(name, args.output)
        finally:
            os.unlink(name)
    print("PASS: simulated hardware protocol only; zero physical/crypto qualification")


def cancelled(*_):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    raise KeyboardInterrupt


if __name__ == "__main__":
    try:
        if Path(sys.argv[0]).name == "monero-wallet-cli":
            raise SystemExit(shim())
        signal.signal(signal.SIGTERM, cancelled)
        signal.signal(signal.SIGINT, cancelled)
        main()
    except (Exception, KeyboardInterrupt) as error:
        print("FAIL:", str(error) if isinstance(error, Refusal) else type(error).__name__)
        raise SystemExit(1)
