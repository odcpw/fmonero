#!/usr/bin/env python3
"""Real, isolated offline Monero wallet qualification. Never uses owner wallets.

The temporary engine generates its own secrets. They stay out of argv, reports
and diagnostic output. The optional regtest mode mines and sends only valueless
coins on a loopback-only, offline fake chain; it never contacts a public daemon.
"""
import argparse
from contextlib import closing
import hashlib
import http.client
import json
import os
from pathlib import Path
import shutil
import shlex
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request


def require(value, label):
    if not value:
        raise RuntimeError(label)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def owner_services():
    return subprocess.run(
        ["systemctl", "--user", "show", "-p", "ActiveState", "-p", "MainPID",
         "fmonero-monerod.service", "fmonero-wallet-rpc.service"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5,
    ).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--regtest", action="store_true", help="also qualify valueless payments and cold signing on an isolated fake chain")
    parser.add_argument("--proxy-policy", action="store_true", help="qualify actual engine proxy inheritance and typed TLS configuration while offline")
    args = parser.parse_args()
    require(not (args.regtest and args.proxy_policy), "proxy-policy and regtest are separate qualifications")
    binary = str(args.binary.resolve())
    engine = shutil.which("monero-wallet-rpc")
    require(engine, "monero-wallet-rpc is required")
    before = owner_services()
    started = time.monotonic()
    checks = []
    process = None
    node_process = None
    with tempfile.TemporaryDirectory(prefix="fmonero-native-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        names = {"FMONERO_STATE_DIR": "state", "FMONERO_CONFIG_DIR": "config",
                 "FMONERO_CACHE_DIR": "cache", "XDG_DATA_HOME": "data",
                 "FMONERO_SYSTEMD_USER_DIR": "units"}
        env = dict(os.environ)
        env.pop("FMONERO_WALLET_PASSWORD", None)
        for name, directory in names.items():
            location = root / directory
            location.mkdir(mode=0o700)
            env[name] = str(location)
        run = root / "state" / "run"
        run.mkdir(mode=0o700)
        wallets = root / "wallets"
        wallets.mkdir(mode=0o700)
        fixture = root / "systemctl.txt"
        env["FMONERO_SYSTEMCTL_FIXTURE"] = str(fixture)
        env["FMONERO_NODES_URL"] = "http://127.0.0.1:1/no-network"
        # Native adapters already pass their private working-directory ringdb.
        # The fake chain deliberately starts the latest fork at height 1. Upstream
        # functional tests use this mismatch flag only for their valueless chain.
        native_cli = shutil.which("monero-wallet-cli")
        require(native_cli, "monero-wallet-cli is required")
        shims = root / "shims"
        shims.mkdir(mode=0o700)
        wrapper = shims / "monero-wallet-cli"
        fixed = [native_cli]
        if args.regtest: fixed.append("--allow-mismatched-daemon-version")
        wrapper.write_text("#!/bin/sh\nexec " + " ".join(shlex.quote(word) for word in fixed) + ' "$@"\n')
        wrapper.chmod(0o700)
        env["PATH"] = str(shims) + os.pathsep + env.get("PATH", "")
        port = free_port()
        url = f"http://127.0.0.1:{port}/json_rpc"
        node_port = free_port()
        proxy_address = f"127.0.0.1:{free_port()}"
        if args.regtest:
            daemon = shutil.which("monerod")
            require(daemon, "monerod is required for isolated regtest")
            node_process = subprocess.Popen([
                daemon, "--regtest", "--offline", "--fixed-difficulty", "1",
                "--data-dir", str(root / "fakechain"), "--disable-dns-checkpoints",
                "--rpc-bind-ip", "127.0.0.1", "--rpc-bind-port", str(node_port),
                "--p2p-bind-ip", "127.0.0.1", "--p2p-bind-port", str(free_port()),
                "--no-zmq", "--non-interactive", "--max-concurrency", "2",
                "--log-file", str(root / "node.log"), "--log-level", "0",
            ], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, start_new_session=True, umask=0o077)
        wallet_mode = (["--daemon-address", f"127.0.0.1:{node_port}", "--trusted-daemon"]
                       if args.regtest else ["--offline", "--stagenet"])
        if args.regtest: wallet_mode.append("--allow-mismatched-daemon-version")
        if args.proxy_policy: wallet_mode += ["--proxy", proxy_address, "--daemon-address", "http://127.0.0.1:1", "--daemon-ssl", "disabled",
                                                     "--daemon-ssl-ca-certificates", "/etc/ssl/certs/ca-certificates.crt"]
        process = subprocess.Popen([
            engine, "--wallet-dir", str(wallets), "--rpc-bind-ip", "127.0.0.1",
            "--rpc-bind-port", str(port), "--rpc-ssl", "disabled",
            "--no-initial-sync", "--non-interactive", *wallet_mode,
            "--shared-ringdb-dir", str(root / "ringdb"),
            "--log-file", str(root / "wallet.log"), "--log-level", "0",
            "--max-concurrency", "2",
        ], cwd=run, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True, umask=0o077)
        fixture.write_text(f"fmonero-wallet-rpc.service active disabled yes {process.pid}\n"
                           "fmonero-monerod.service inactive disabled no -\n")
        fixture.chmod(0o600)

        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
        authentication = None
        username = credential_password = ""
        def rpc(method, params=None):
            nonlocal authentication, username, credential_password
            if not username:
                credential = run / f"monero-wallet-rpc.{port}.login"
                require(credential.stat().st_mode & 0o077 == 0, "scratch RPC credentials are not private")
                username, credential_password = credential.read_text().strip().split(":", 1)
            body = json.dumps({"jsonrpc": "2.0", "id": "native", "method": method, "params": params or {}}).encode()
            # Monero's Digest nonce belongs to its TCP session. urllib closes the
            # 401 connection, so a retry receives another nonce and cannot authenticate.
            # Keep one real HTTP connection and nonce counter for this scratch client.
            for attempt in range(2):
                headers = {"Content-Type": "application/json"}
                if authentication:
                    auth = authentication
                    auth["count"] += 1
                    count = f'{auth["count"]:08x}'
                    cnonce = os.urandom(16).hex()
                    digest = lambda value: hashlib.md5(value.encode(), usedforsecurity=False).hexdigest()
                    first = digest(f'{username}:{auth["realm"]}:{credential_password}')
                    second = digest("POST:/json_rpc")
                    response_hash = digest(f'{first}:{auth["nonce"]}:{count}:{cnonce}:auth:{second}')
                    quoted = lambda value: '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'
                    headers["Authorization"] = "Digest " + ", ".join([
                        "username=" + quoted(username), "realm=" + quoted(auth["realm"]),
                        "nonce=" + quoted(auth["nonce"]), 'uri="/json_rpc"',
                        "response=" + quoted(response_hash), "algorithm=MD5", "qop=auth",
                        "nc=" + count, "cnonce=" + quoted(cnonce),
                    ] + (["opaque=" + quoted(auth["opaque"])] if auth.get("opaque") else []))
                connection.request("POST", "/json_rpc", body, headers)
                response = connection.getresponse()
                raw = response.read(4 * 1024 * 1024 + 1)
                if response.status != 401:
                    require(response.status == 200, "scratch RPC returned an unexpected HTTP status")
                    break
                choices = [urllib.request.parse_keqv_list(urllib.request.parse_http_list(header[7:]))
                    for header in response.headers.get_all("WWW-Authenticate", []) if header.startswith("Digest ")]
                selected = next((choice for choice in choices if choice.get("algorithm", "MD5").upper() == "MD5"
                    and "auth" in choice.get("qop", "").split(",")), None)
                require(selected and "realm" in selected and "nonce" in selected, "scratch Digest policy was unsupported")
                selected["count"] = 0
                authentication = selected
            else:
                raise RuntimeError("scratch Digest authentication failed")
            require(len(raw) <= 4 * 1024 * 1024, "native response exceeded its bound")
            document = json.loads(raw)
            require("error" not in document, "native backend rejected " + method)
            return document["result"]

        def cli(words, secret_input=None, code=0):
            cli_process = subprocess.Popen([binary, *words, "--json"], env=env,
                stdin=subprocess.PIPE, text=True, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, start_new_session=True)
            try:
                stdout, stderr = cli_process.communicate(secret_input, timeout=60)
            except subprocess.TimeoutExpired:
                # Let fmonero cancel/reap its separately supervised native child.
                cli_process.send_signal(signal.SIGTERM)
                try:
                    cli_process.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(cli_process.pid, signal.SIGKILL)
                    cli_process.communicate(timeout=5)
                raise RuntimeError("scratch CLI exceeded its qualification deadline")
            result = subprocess.CompletedProcess(cli_process.args, cli_process.returncode, stdout, stderr)
            if result.returncode != code:
                label = " ".join(words[:2])
                if len(words) > 3 and words[:2] == ["wallet", "tools"]:
                    label += " " + words[2] + " " + words[3]
                try:
                    reason = json.loads(result.stdout).get("reason", "")
                except ValueError:
                    reason = "invalid_json"
                if reason not in {"", "invalid_json", "unreachable", "bad_args", "bad_password", "wallet_peer_unverified", "transport_policy", "killed_at_deadline", "needs_yes", "wallet_changed"}:
                    reason = "classified_error"
                if words[:4] == ["wallet", "tools", "offline", "submit"]:
                    outcome = json.loads(result.stdout).get("result", {})
                    print("Scratch native submission evidence:", json.dumps({
                        "all_submitted": outcome.get("all_submitted"),
                        "unresolved": outcome.get("unresolved"),
                        "proven_hashes": len(outcome.get("txids", [])),
                        "wallet_reopened": outcome.get("returned", {}).get("wallet_reopened"),
                        "warning": outcome.get("warning"),
                    }))
                raise RuntimeError(f"CLI result failed for {label}: exit {result.returncode}, reason {reason}")
            require(len(result.stdout) <= 4 * 1024 * 1024, "CLI output exceeded bound")
            if secret_input:
                for secret in secret_input.splitlines():
                    if secret:
                        require(secret not in result.stdout + result.stderr, "secret appeared in CLI output")
            return json.loads(result.stdout)

        def node_rpc(method, params=None):
            require(node_process is not None and node_process.poll() is None,
                    "isolated fake-chain daemon exited")
            with closing(http.client.HTTPConnection("127.0.0.1", node_port, timeout=30)) as node:
                body = json.dumps({"jsonrpc": "2.0", "id": "regtest", "method": method,
                                   "params": params or {}})
                path = "/get_transaction_pool" if method == "get_transaction_pool" else "/json_rpc"
                node.request("POST", path, "{}" if method == "get_transaction_pool" else body, {"Content-Type": "application/json"})
                response = node.getresponse()
                raw = response.read(4 * 1024 * 1024 + 1)
                require(response.status == 200 and len(raw) <= 4 * 1024 * 1024,
                        "fake-chain RPC response exceeded its policy")
                document = json.loads(raw)
                require("error" not in document, "fake-chain backend rejected " + method)
                return document if method == "get_transaction_pool" else document["result"]

        try:
            deadline = time.monotonic() + 15
            while True:
                require(process.poll() is None, "temporary Monero engine exited")
                try:
                    version = rpc("get_version")["version"]
                    break
                except (OSError, RuntimeError) as failure:
                    cause = str(failure) if isinstance(failure, RuntimeError) else type(failure).__name__ + " " + str(getattr(failure, "code", ""))
                    require(time.monotonic() < deadline, "temporary Monero engine did not start: " + cause)
                    time.sleep(0.05)
            for key, value in [("wallet.url", f"http://127.0.0.1:{port}"),
                               ("wallet.port", str(port)), ("wallet.dir", str(wallets)),
                               ("node.network", "mainnet" if args.regtest else "stagenet"),
                               ("node.remote", f"127.0.0.1:{node_port}" if args.regtest else "127.0.0.1:1"),
                               ("node.remote_trusted", "true" if args.regtest else "false"),
                               ("node.mode", "remote")]:
                cli(["settings", "set", key, value])
            if args.proxy_policy:
                cli(["settings", "set", "node.proxy", proxy_address])
            # Native generation and query remain private within the temporary process.
            password = "scratch-" + os.urandom(16).hex()
            rpc("create_wallet", {"filename": "source", "password": password, "language": "English"})
            original = rpc("get_address", {"account_index": 0})["address"]
            seed = rpc("query_key", {"key_type": "mnemonic"})["key"]
            rpc("close_wallet")
            restored = cli(["wallet", "restore", "restored", "--restore-height", "0",
                "--secret-stdin", "--yes"], seed + "\n" + password + "\n")
            require(restored.get("state") == "ready", "restore did not finish setup")
            address = cli(["wallet", "address"])["primary"]
            require(address == original, "restored wallet identity differed")
            checks.append("native_seed_restore_preserves_identity_and_opens_wallet")
            balance = cli(["wallet", "balance"])
            require(balance["balance_pico"] == 0 and balance["unlocked_pico"] == 0,
                    "empty scratch wallet balance differed")
            checks.append("native_exact_empty_balance")
            new = cli(["wallet", "new-address", "--label", "Scratch receive"])
            require(new["selected"] != original and any(row["address"] == new["selected"]
                and row["label"] == "Scratch receive" for row in new["addresses"]),
                "new subaddress was not the actual labeled backend row")
            checks.append("native_labeled_subaddress_creation")
            valid = cli(["wallet", "validate", new["selected"]])
            require(valid.get("valid") is True, "native subaddress validation failed")
            checks.append("native_subaddress_validation")
            request = cli(["wallet", "request", "create", "--address", new["selected"],
                "--amount", "0.000000000001", "--description", "café & exact bytes"])
            parsed = cli(["wallet", "request", "parse", request["uri"]])
            require(parsed["amount_pico"] == 1 and parsed["description"] == "café & exact bytes",
                    "payment request roundtrip changed amount or text")
            checks.append("native_address_payment_request_exact_integer_unicode_roundtrip")
            account = cli(["wallet", "accounts", "create", "Scratch account"])
            index = account["account_index"]
            cli(["wallet", "accounts", "rename", str(index), "Renamed scratch account"])
            cli(["wallet", "accounts", "select", str(index)])
            snapshot = cli(["wallet", "accounts", "list"])
            require(snapshot["selected"] == index and any(row["account_index"] == index
                and row["label"] == "Renamed scratch account" for row in snapshot["accounts"]),
                "native account create/rename/select differed")
            checks.append("native_account_create_rename_select")
            contact = cli(["wallet", "book", "add", new["selected"], "Scratch recipient"])
            cli(["wallet", "book", "edit", str(contact["index"]), new["selected"], "Edited recipient", "--yes"])
            book = cli(["wallet", "book", "list"])
            require(any(row["index"] == contact["index"] and row["description"] == "Edited recipient"
                for row in book["entries"]), "native contact edit did not affect reviewed row")
            cli(["wallet", "book", "remove", str(contact["index"]), "--yes"])
            require(not cli(["wallet", "book", "list"])["entries"], "native contact removal failed")
            checks.append("native_contact_add_edit_remove")
            data = root / "message.txt"
            signature = root / "message.signature"
            data.write_text("Exact UTF-8 message: café\nA trailing newline matters.\n")
            data.chmod(0o600)
            cli(["wallet", "tools", "message", "sign", str(data), "--output", str(signature),
                 "--account", "0", "--index", "0", "--yes"])
            verification = cli(["wallet", "tools", "message", "verify", original, str(data), str(signature)])
            require(verification["good"] is True, "native message signature did not verify")
            require(signature.stat().st_mode & 0o777 == 0o600, "signature artifact permissions were not private")
            data.write_text("A changed message must fail.\n")
            invalid = cli(["wallet", "tools", "message", "verify", original, str(data), str(signature)], code=1)
            require(invalid["good"] is False, "changed signed message was falsely accepted")
            checks.append("native_message_sign_verify_tamper_rejection_private_artifact")
            history_path = root / "history.json"
            cli(["wallet", "tools", "care", "export-history", str(history_path), "--format", "json", "--yes"])
            require(json.loads(history_path.read_text()) == [], "empty native history export differed")
            require(history_path.stat().st_mode & 0o777 == 0o600, "history artifact permissions were not private")
            checks.append("native_exact_empty_history_private_export")
            backup_password = "backup-" + os.urandom(16).hex()
            backup_directory = root / "recovery-backup"
            backup = cli(["wallet", "tools", "care", "backup", str(backup_directory),
                "--secret-stdin", "--yes"], backup_password + "\n")
            require(backup.get("password_reopened") is True and backup.get("address") == original,
                    "native recovery wallet did not independently reopen with the expected identity")
            require(backup["labels_included"] is False and backup["history_included"] is False,
                    "recovery metadata coverage was overstated")
            checks.append("native_encrypted_recovery_generation_password_reopen_identity")
            outputs = root / "outputs.json"
            images = root / "images.json"
            cli(["wallet", "tools", "care", "export-outputs", str(outputs), "--yes"])
            image_shape = rpc("export_key_images", {"all": True})
            require(image_shape.get("offset") == 0 and not image_shape.get("signed_key_images", []),
                    "empty native key-image response differed")
            cli(["wallet", "tools", "care", "export-key-images", str(images), "--yes"])
            require(outputs.stat().st_mode & 0o777 == 0o600 and images.stat().st_mode & 0o777 == 0o600,
                    "exchange artifacts were not private")
            checks.append("native_output_key_image_private_exports")
            view_key = rpc("query_key", {"key_type": "view_key"})["key"]
            view_password = "view-" + os.urandom(16).hex()
            view = cli(["wallet", "tools", "care", "view-only", "incoming", original,
                "--secret-stdin", "--yes"], view_key + "\n" + view_password + "\n")
            require(view["state"] == "ready", "native view-only creation did not finish")
            require(cli(["wallet", "accounts", "list"])["selected"] == 0,
                    "new view-only wallet retained invalid previous account selection")
            capability = cli(["wallet", "tools", "care", "capabilities"])
            require(capability["kind_hint"] == "view_only" and capability["spend_authority"] is None,
                    "view-only capability evidence was overstated")
            imported = cli(["wallet", "tools", "care", "import-outputs", str(outputs), "--yes"])
            require(imported["outputs_imported"] == 0, "empty output import differed")
            checks.append("native_view_only_creation_output_exchange_account_reset")
            cli(["wallet", "close"])
            cli(["wallet", "open", "restored", "--password-stdin"], password + "\n")
            new_password = "changed-" + os.urandom(16).hex()
            cli(["wallet", "tools", "care", "password", "--secret-stdin", "--yes"],
                password + "\n" + new_password + "\n")
            cli(["wallet", "close"])
            cli(["wallet", "open", "restored", "--password-stdin"], password + "\n", code=9)
            cli(["wallet", "open", "restored", "--password-stdin"], new_password + "\n")
            require(cli(["wallet", "address"])["primary"] == original,
                    "password change did not preserve the reopened wallet identity")
            checks.append("native_password_change_old_password_refusal_new_password_reopen_identity")
            if args.proxy_policy:
                configured_connection = cli(["wallet", "use-node", "https://127.0.0.1:1", "--yes"])
                require(configured_connection["ssl_support"] == "enabled" and configured_connection["proxy"] == proxy_address,
                        "native HTTPS configuration did not require TLS and attest the inherited proxy")
                cli(["settings", "set", "node.fingerprint", "ab" * 32])
                pinned = cli(["wallet", "use-node", "https://127.0.0.1:1", "--yes"])
                require(pinned["ssl_support"] == "enabled", "native fingerprint configuration did not retain required TLS")
                cli(["settings", "set", "node.proxy", f"127.0.0.1:{free_port()}"])
                cli(["wallet", "use-node", "https://127.0.0.1:1", "--yes"], code=3)
                checks.append("native_owned_proxy_attestation_https_pin_set_daemon_ack_and_mismatch_refusal_no_contact")
                cli(["settings", "set", "node.proxy", proxy_address])
            if args.regtest:
                # Fake-chain coins have no value and exist only in this scratch directory.
                node_rpc("generateblocks", {"amount_of_blocks": 80, "wallet_address": original})
                cli(["wallet", "refresh"])
                require(cli(["wallet", "balance", "--account", "0"])["unlocked_pico"] > 10**12,
                        "fake-chain rewards did not unlock")
                cli(["wallet", "accounts", "select", "0"])
                recipient = new["selected"]
                # Native event detection, with a private helper simulating failed
                # desktop acknowledgement. Never send test alerts to the owner.
                alert_log = root / "alert-helper.jsonl"
                alert_helper = shims / "notify-send"
                alert_helper.write_text("#!/usr/bin/python3\nimport json,sys\nwith open(" + repr(str(alert_log)) +
                    ", 'a') as output: output.write(json.dumps(sys.argv[1:])+'\\n')\nsys.exit(1)\n")
                alert_helper.chmod(0o700)
                cli(["wallet", "notifications", "enable"])
                require(cli(["wallet", "notifications", "poll"])["claimed"] == 0,
                        "first native alert poll replayed historical activity")
                preview = cli(["wallet", "send", recipient, "0.01", "--account", "0", "--dry-run"])
                require(preview.get("amount_pico") == 10**10 and preview.get("relayed") is False,
                        "actual send preview amount or relay status differed")
                cli(["wallet", "send", recipient, "0.01", "--account", "0", "--max-fee", "1p", "--yes"], code=3)
                require(node_rpc("get_transaction_pool") .get("transactions", []) == [],
                        "preview or fee refusal relayed a transaction")
                sent = cli(["wallet", "send", recipient, "0.01", "--account", "0", "--yes"])
                require(sent.get("txids") and all(len(txid) == 64 for txid in sent["txids"]),
                        "actual fake-chain send had no backend transaction receipt")
                txid = sent["txids"][0]
                node_rpc("generateblocks", {"amount_of_blocks": 5, "wallet_address": original})
                cli(["wallet", "refresh"])
                checks.append("regtest_exact_send_preview_fee_refusal_single_relay_and_confirmation")
                cli(["wallet", "notifications", "poll"], code=1)
                alert_args = json.loads(alert_log.read_text())
                require(len(alert_args) == 4 and alert_args[:3] == ["--app-name=fmonero", "--", "Incoming Monero payment"],
                        "desktop helper did not receive the generic fixed notification route")
                require(original not in str(alert_args) and recipient not in str(alert_args) and "XMR" not in str(alert_args),
                        "notification payload exposed payment details")
                require(cli(["wallet", "notifications", "poll"])["claimed"] == 0 and
                        cli(["wallet", "notifications", "poll"])["claimed"] == 0,
                        "failed desktop acknowledgement replayed native candidates across CLI restarts")
                cli(["wallet", "notifications", "disable"])
                checks.append("regtest_native_alert_baseline_generic_payload_failed_helper_ack_no_replay")
                cli(["wallet", "tools", "care", "rescan", "--spent", "--yes"])
                cli(["wallet", "tools", "care", "rescan", "--yes"])
                cli(["wallet", "refresh"])
                require(cli(["wallet", "address"])["primary"] == original and
                        cli(["wallet", "balance", "--account", "0"])["balance_pico"] > 0,
                        "native rescan did not preserve the scratch identity and reconstruct activity")
                checks.append("regtest_trusted_spent_full_rescan_reconstructs_identity_and_activity")
                transaction_proof = root / "payment-proof.txt"
                cli(["wallet", "tools", "proof", "transaction", txid, recipient,
                     "--output", str(transaction_proof), "--yes"])
                proven = cli(["wallet", "tools", "proof", "check-transaction", txid, recipient,
                              str(transaction_proof)])
                require(proven["good"] is True and proven["received_pico"] == 10**10,
                        "real transaction proof amount or validity differed")
                spend_proof = root / "spend-proof.txt"
                cli(["wallet", "tools", "proof", "spend", txid, "--output", str(spend_proof), "--yes"])
                require(cli(["wallet", "tools", "proof", "check-spend", txid, str(spend_proof)])["good"] is True,
                        "real spend proof did not verify")
                reserve_proof = root / "reserve-proof.txt"
                cli(["wallet", "tools", "proof", "reserve", "--amount", "1", "--account", "0",
                     "--output", str(reserve_proof), "--yes"])
                require(cli(["wallet", "tools", "proof", "check-reserve", original, str(reserve_proof)])["good"] is True,
                        "real reserve proof did not verify")
                for kind, proof_args in [
                    ("check-transaction", [txid, recipient, str(transaction_proof)]),
                    ("check-spend", [txid, str(spend_proof)]),
                    ("check-reserve", [original, str(reserve_proof)]),
                ]:
                    wrong = cli(["wallet", "tools", "proof", kind, *proof_args,
                                 "--message", str(data)], code=1)
                    require(wrong["good"] is False, "proof accepted a changed message scope")
                checks.append("regtest_transaction_spend_reserve_proofs_exact_amount_and_scope_tamper")
                funded_outputs, funded_images = root / "funded-outputs.json", root / "funded-images.json"
                cli(["wallet", "tools", "care", "export-outputs", str(funded_outputs), "--yes"])
                cli(["wallet", "tools", "care", "export-key-images", str(funded_images), "--yes"])
                cli(["wallet", "close"])
                cli(["wallet", "open", "incoming", "--password-stdin"], view_password + "\n")
                cli(["wallet", "refresh"])
                cli(["wallet", "tools", "care", "import-key-images", str(funded_images), "--yes"])
                unsigned = root / "unsigned.monero"
                prepared = cli(["wallet", "tools", "offline", "prepare", recipient, "0.02",
                    "--account", "0", "--output", str(unsigned), "--yes"])
                require(prepared["amount"] == 2 * 10**10 and unsigned.read_bytes().startswith(b"Monero unsigned tx set"),
                        "cold preparation did not produce an exact native unsigned artifact")
                cli(["wallet", "close"])
                cli(["wallet", "open", "restored", "--password-stdin"], new_password + "\n")
                description = cli(["wallet", "tools", "offline", "describe", str(unsigned)])
                require(sum(row["fee"] for row in description["transactions"]) == prepared["fee"],
                        "native unsigned description fee differed")
                signed = root / "signed.monero"
                declined = cli(["wallet", "tools", "offline", "sign", str(unsigned), "--output", str(signed),
                                "--secret-stdin"], new_password + "\n", code=3)
                require(declined["signed"] is False and declined["returned"]["wallet_reopened"] is True and not signed.exists(),
                        "declining native signing did not return the original wallet intact")
                signed_result = cli(["wallet", "tools", "offline", "sign", str(unsigned), "--output", str(signed),
                                     "--secret-stdin", "--yes"], new_password + "\n")
                require(signed_result["signed"] is True and signed.read_bytes().startswith(b"Monero signed tx set"),
                        "native signing did not produce the actual signed artifact")
                submitted = cli(["wallet", "tools", "offline", "submit", str(signed), "--secret-stdin", "--yes"],
                                new_password + "\n")
                require(submitted["result"]["all_submitted"] is True and submitted["result"]["txids"],
                        "native fake-chain submission was not fully confirmed")
                checks.append("regtest_view_only_key_images_native_prepare_describe_decline_sign_submit")
                # Inject a valid unresolved ledger for the just-submitted artifact
                # to qualify observation recovery. This is a software fault case,
                # not evidence of an actual network interruption or power failure.
                rpc("close_wallet")
                pending_path = root / "state" / "native-pending.json"
                injected = {"wallet": {"name": "restored", "network": "mainnet", "primary_address": original},
                            "artifact_sha256": hashlib.sha256(signed.read_bytes()).hexdigest(),
                            "transactions": 1, "unknown_txids": 1, "proven_txids": [],
                            "attempted_at": int(time.time())}
                pending_path.write_text(json.dumps(injected))
                pending_path.chmod(0o600)
                cli(["wallet", "open", "restored", "--password-stdin"], new_password + "\n", code=3)
                observed = cli(["wallet", "tools", "offline", "inspect-pending", "--secret-stdin", "--yes"],
                               new_password + "\n")
                require(observed["inspection_only"] is True and observed["returned"]["wallet_reopened"] is True,
                        "native pending inspection did not reopen the recorded wallet")
                require(json.loads(pending_path.read_text()) == injected,
                        "inspection modified the unresolved ledger")
                cli(["wallet", "new-address", "--label", "blocked"], code=3)
                cli(["wallet", "tools", "offline", "clear-pending", "--yes"])
                require(not pending_path.exists(), "explicit native acknowledgement did not clear the ledger")
                checks.append("regtest_injected_native_pending_inspection_reopens_identity_retains_payment_guard")
            # Existing-name refusal occurs before the CLI tries to read a secret.
            cli(["wallet", "restore", "restored", "--secret-stdin", "--yes"], code=3)
            checks.append("restore_refuses_existing_wallet_before_secret_input")
            cli(["settings", "set", "tui.privacy", "true"])
            require(cli(["app", "status"]).get("privacy") is True, "local shared privacy absent")
            checks.append("shared_privacy_in_local_app_status")
            # Close native wallet explicitly; all owner services remain independent.
            cli(["wallet", "close"])
            cli(["wallet", "notifications", "disable"])
            require(cli(["wallet", "notifications", "poll"])["claimed"] == 0,
                    "disabled notifications claimed observations")
            checks.append("disabled_closed_notifications_claim_nothing")
            require(owner_services() == before, "owner service state changed")
            checks.append("owner_service_state_unchanged")
            seed = password = new_password = view_key = view_password = backup_password = credential_password = ""
            report = {"schema": 1, "engine_rpc_version": version,
                      "binary_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
                      "scope": ("isolated valueless fake-chain software interoperability; no public-network funded or physical hardware qualification" if args.regtest else "isolated native proxy/TLS configuration acknowledgements; no remote contact or certificate handshake" if args.proxy_policy else "isolated offline Monero wallet, no payment or hardware qualification"),
                      "checks": checks, "elapsed_sec": round(time.monotonic() - started, 3)}
        finally:
            connection.close()
            for owned_process in (process, node_process):
                if owned_process is not None and owned_process.poll() is None:
                    os.killpg(owned_process.pid, signal.SIGTERM)
                    try:
                        owned_process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(owned_process.pid, signal.SIGKILL)
                        owned_process.wait(timeout=5)

        require(owner_services() == before, "owner service state changed after scratch cleanup")
        report["cleanup_complete"] = True
        report["owner_service_state_unchanged"] = True
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        args.output.chmod(0o600)
        print(f"PASS: {len(checks)} native {'regtest' if args.regtest else 'transport' if args.proxy_policy else 'offline'} wallet checks")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError, KeyError) as error:
        # Do not print raw protocol payloads, subprocess output or generated secrets.
        detail = str(error) if isinstance(error, RuntimeError) else type(error).__name__
        if isinstance(error, KeyError) and error.args and error.args[0] in {"realm", "nonce", "count", "result", "version", "key", "address", "primary", "account_index", "selected", "good", "entries"}:
            detail += " missing field " + error.args[0]
        print("FAIL:", detail)
        raise SystemExit(1)
