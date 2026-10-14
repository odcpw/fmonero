#!/usr/bin/env python3
"""Run the real QML Service in an isolated windowless shell and change local policy."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    plugin = Path(__file__).resolve().parents[1] / "plugin" / "oliver.monero"
    with tempfile.TemporaryDirectory(prefix="fmonero-policy-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        env.pop("FMONERO_WALLET_PASSWORD", None)
        for key, folder in {"FMONERO_STATE_DIR": "state", "FMONERO_CONFIG_DIR": "config",
            "FMONERO_CACHE_DIR": "cache", "XDG_DATA_HOME": "data",
            "FMONERO_SYSTEMD_USER_DIR": "units"}.items():
            path = root / folder
            path.mkdir(mode=0o700)
            env[key] = str(path)
        fixture = root / "systemctl.txt"
        fixture.write_text("")
        env["FMONERO_SYSTEMCTL_FIXTURE"] = str(fixture)
        env["FMONERO_NODES_URL"] = "http://127.0.0.1:1/no-network"
        env["QT_QPA_PLATFORM"] = "offscreen"

        def setting(key, value):
            result = subprocess.run([binary, "settings", "set", key, value, "--json"], env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            if result.returncode:
                raise RuntimeError("isolated setting update failed")

        setting("tui.privacy", "true")
        qml = root / "shell.qml"
        qml.write_text('''import QtQuick
import Quickshell
import Quickshell.Io
import "''' + plugin.as_uri() + '''" as Plugin
ShellRoot {
  Plugin.Service { id: svc; settings: ({ binary: ''' + json.dumps(binary) + ''' }) }
  IpcHandler {
    target: "fmoneroPolicyTest"
    function state(): string {
      return JSON.stringify({ privacy: svc.privacy, known: svc._privacyKnown,
        notifications: svc.notificationsEnabled, appRunning: svc.appRunning,
        policyEpoch: svc._policyEpoch })
    }
  }
}
''')
        log = root / "shell.log"
        with log.open("wb") as output:
            process = subprocess.Popen(["quickshell", "--path", str(qml), "--no-color"],
                env=env, stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                start_new_session=True)
            try:
                def wait_for(privacy, notifications, previous_epoch=None):
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        if process.poll() is not None:
                            # Logs contain only isolated config and fixed process errors.
                            raise RuntimeError("windowless QML service exited: " + log.read_text()[-1500:])
                        result = subprocess.run(["quickshell", "ipc", "--path", str(qml),
                            "call", "fmoneroPolicyTest", "state"], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=3)
                        try:
                            state = json.loads(result.stdout)
                            if isinstance(state, str):
                                state = json.loads(state)
                        except ValueError:
                            time.sleep(0.05)
                            continue
                        if state.get("known") and state.get("privacy") is privacy and state.get("notifications") is notifications:
                            if previous_epoch is not None and state["policyEpoch"] <= previous_epoch:
                                raise RuntimeError("policy changed only through periodic polling, not the file watcher")
                            if state.get("appRunning"):
                                raise RuntimeError("closed isolated app unexpectedly became active")
                            return state
                        time.sleep(0.05)
                    raise RuntimeError("shared presentation did not update from the atomic config change")

                state = wait_for(True, False)
                for privacy, notifications in [(False, False), (True, False), (True, True), (True, False)]:
                    epoch = state["policyEpoch"]
                    setting("tui.privacy", str(privacy).lower())
                    setting("tui.notifications", str(notifications).lower())
                    state = wait_for(privacy, notifications, epoch)
                print("PASS: real windowless QML shared-policy initial masking, atomic replacement, repeat changes and closed lifecycle")
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print("FAIL:", str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1)
