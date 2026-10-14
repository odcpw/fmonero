//! In-process tests against `fmonero-mock` — the §2a money tests and the §2c Digest tests.
//!
//! **The mock's source is included, not reimplemented.** §8 Phase 0 asks for one
//! implementation of the request parsing, the Digest server and the request log, shared by
//! "making the binary a thin `main()` over a `serve(routes)` function". Sharing it as a
//! library would need a `[dev-dependencies]` entry in `crates/fmonero-core/Cargo.toml`, and
//! **no `Cargo.toml` may be edited after the 1a gate** (§2k) — so it is included with
//! `#[path]` and driven through `mock::spawn_for_test`, which binds an ephemeral port. The
//! deviation is recorded in DECISIONS.md.
//!
//! Every test here is an AMBER row (§4b): it proves fmonero's wire format and its parsing
//! against fixtures **derived from the Monero source**, and proves nothing about a real
//! daemon.

#[path = "../../../fmonero-mock/src/main.rs"]
#[allow(dead_code)]
mod mock;

use std::path::PathBuf;
use std::time::Duration;

use crate::amount::Piconero;
use crate::context::Context;
use crate::proc::CancelToken;
use crate::send::{self, SendRequest, SweepRequest};
use crate::state::{self, PendingKind, PendingSend, PendingTx};

/// The mock's own credential, and the one written to the login file (§2c). There is **no
/// unauthenticated wallet path that works**: a test that omits the file is testing exit 4.
const AUTH: &str = "fmonero:t3st";

/// `tests/fixtures`, from this crate's manifest directory.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .expect("tests/fixtures must exist: Phase 0 wrote it")
}

/// One test's world: its own state directory, its own mocks, its own request logs.
struct World {
    dir: PathBuf,
    ctx: Context,
    node_log: PathBuf,
    wallet_log: PathBuf,
}

impl World {
    /// A node mock and a wallet mock, each with the extra argv the test needs, and the
    /// matching login file written **before** any client is built (§2c).
    fn new(name: &str, node_extra: &[&str], wallet_extra: &[&str]) -> World {
        let dir = std::env::temp_dir().join(format!(
            "fmonero-mock-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = Context::for_test(&dir).expect("test context");
        let fixtures = fixtures().display().to_string();
        let node_log = dir.join("node.jsonl");
        let wallet_log = dir.join("wallet.jsonl");

        let mut node_args: Vec<String> = vec![
            "--role".into(),
            "node".into(),
            "--port".into(),
            "0".into(),
            "--fixtures".into(),
            fixtures.clone(),
            "--log".into(),
            node_log.display().to_string(),
        ];
        node_args.extend(node_extra.iter().map(|s| (*s).to_string()));
        let node_port = spawn(&node_args);

        let mut wallet_args: Vec<String> = vec![
            "--role".into(),
            "wallet".into(),
            "--port".into(),
            "0".into(),
            "--fixtures".into(),
            fixtures,
            "--log".into(),
            wallet_log.display().to_string(),
            "--auth".into(),
            AUTH.into(),
        ];
        wallet_args.extend(wallet_extra.iter().map(|s| (*s).to_string()));
        let wallet_port = spawn(&wallet_args);

        ctx.settings.node.url = format!("http://127.0.0.1:{node_port}");
        ctx.settings.wallet.url = format!("http://127.0.0.1:{wallet_port}");
        // The login file is named after the BIND PORT (§2c, §4.2), so the two must agree or
        // `WalletClient::from_ctx` looks for a file the daemon never wrote.
        ctx.settings.wallet.port = wallet_port;
        ctx.settings.wallet.dir = dir.join("wallets/mainnet");
        std::fs::create_dir_all(&ctx.paths.run_dir).expect("run dir");
        write_login(&ctx.paths.wallet_login_file(wallet_port));
        World {
            dir,
            ctx,
            node_log,
            wallet_log,
        }
    }

    fn wallet(&self) -> crate::rpc::WalletClient {
        crate::rpc::WalletClient::from_ctx(&self.ctx).expect("wallet client")
    }

    fn node(&self) -> crate::rpc::NodeClient {
        crate::rpc::NodeClient::from_ctx(&self.ctx).expect("node client")
    }

    /// Every recorded request body for one method, in order.
    ///
    /// A json_rpc call's arguments are `body.params`; a **legacy** `POST /<name>` has no
    /// envelope at all and its body IS the arguments (§1b.2), so both shapes land here.
    fn bodies(&self, log: &PathBuf, method: &str) -> Vec<serde_json::Value> {
        read_log(log)
            .into_iter()
            .filter(|l| l["method"] == method)
            .map(|l| match l["body"].get("params") {
                Some(p) => p.clone(),
                None => l["body"].clone(),
            })
            .collect()
    }

    fn count(&self, log: &PathBuf, method: &str) -> usize {
        read_log(log)
            .iter()
            .filter(|l| l["method"] == method)
            .count()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn spawn(args: &[String]) -> u16 {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    mock::spawn_for_test(&refs).expect("the mock must start")
}

/// `printf 'fmonero:t3st' > …/monero-wallet-rpc.<port>.login; chmod 600` — the three lines
/// §2c says every test that reaches the wallet needs.
fn write_login(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, AUTH.as_bytes()).expect("login file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("0600");
}

fn read_log(path: &PathBuf) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn a_send(dest: &str) -> SendRequest {
    SendRequest {
        dest: dest.to_string(),
        amount: Piconero(100_000_000_000),
        account_index: 0,
        priority: 2,
        note: None,
        max_fee: None,
    }
}

/// A real, checksum-valid mainnet address from upstream's own test vectors — the same one
/// `validate_address-standard.json` answers for (`_request_address`).
const DEST: &str = "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm";
/// The txid `transfer_split-do-not-relay.json` builds and `relay_tx.json` echoes.
const BUILT_TXID: &str = "a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f";

// ---- §2a.3: exactly one transfer and one relay_tx, with identical metadata ----

/// The central §2a test: **the send path issues exactly one `transfer_split` and one `relay_tx`, and
/// the `relay_tx.hex` is byte-identical to the `transfer_split` response's `tx_metadata`.**
///
/// `relay_tx`'s `hex` is hex of a serialized `wallet2::pending_tx` — **not** `tx_blob`
/// (§1b.4) — so relaying anything else is a send that silently does nothing.
#[test]
fn the_send_path_builds_once_and_relays_that_exact_transaction() {
    let w = World::new("send-once", &[], &[]);
    let client = w.wallet();
    let node = w.node();
    let cancel = CancelToken::new();
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(DEST),
        &cancel,
    )
    .expect("the build must succeed against the fixtures");
    assert_eq!(plan.txs.len(), 1);
    assert_eq!(plan.txs[0].txid, BUILT_TXID);
    // The fee is the fixture's real, non-zero fee — never an estimate fmonero computed (§2a.2).
    assert_eq!(plan.total_fee, Piconero(30_720_000));
    assert_eq!(plan.total_amount, Piconero(100_000_000_000));
    let metadata = plan.txs[0].metadata.clone();

    let announced = std::sync::Mutex::new(Vec::<String>::new());
    let txids = send::relay(
        &w.ctx,
        &client,
        plan,
        &|s| announced.lock().unwrap().push(s.to_string()),
        &cancel,
    )
    .expect("the relay must succeed");
    assert_eq!(txids, vec![BUILT_TXID.to_string()]);

    assert_eq!(
        w.count(&w.wallet_log, "transfer_split"),
        1,
        "exactly one build"
    );
    assert_eq!(w.count(&w.wallet_log, "relay_tx"), 1, "exactly one relay");
    let relayed = w.bodies(&w.wallet_log, "relay_tx");
    assert_eq!(
        relayed[0]["hex"].as_str().unwrap(),
        metadata,
        "relay_tx.hex must be the metadata that very transfer returned (§2a.3)"
    );
    // §2a.4: one `store` after the last relay, so a SIGKILL cannot lose the wallet's own
    // record of the outgoing transfer.
    assert_eq!(w.count(&w.wallet_log, "store"), 1);
    // §2a.2: the number that is paid is announced before the first relay_tx.
    let lines = announced.lock().unwrap().clone();
    assert!(!lines.is_empty(), "relay must announce");
    assert!(lines[0].contains("0.1000"), "{lines:?}");
    assert!(lines[0].contains(DEST), "{lines:?}");
    assert!(lines[0].contains("fee"), "{lines:?}");
    // A normal successful send leaves nothing behind (§2a.4).
    assert!(state::pending_send(&w.ctx).unwrap().is_none());
}

/// **Round 4, S2: the amount the wallet service prepared must be the amount that was asked for.**
///
/// Until this check the confirm screen a person types `send` into rendered `plan.total_amount` —
/// the service's own reply — while the form two columns away showed what he had typed, and nothing
/// anywhere compared the two. `--scenario amount-mismatch` is a `transfer_split` answer for 0.2 XMR to a
/// request for 0.1, which no correct wallet-rpc gives; the build refuses with exit 3, names both
/// figures, and nothing is relayed.
#[test]
fn a_build_that_comes_back_with_a_different_amount_is_refused_before_anything_is_sent() {
    let w = World::new("amount-mismatch", &[], &["--scenario", "amount-mismatch"]);
    let client = w.wallet();
    let node = w.node();
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect_err("a plan for the wrong amount must not be returned");
    assert_eq!(e.exit_code(), 3, "{e}");
    assert_eq!(e.reason(), Some("amount_mismatch"), "{e}");
    let text = e.to_string();
    assert!(text.contains("0.1000"), "the amount asked for: {text}");
    assert!(text.contains("0.2000"), "the amount prepared: {text}");
    assert!(text.contains("Nothing was sent"), "{text}");
    assert_eq!(
        w.count(&w.wallet_log, "relay_tx"),
        0,
        "a refused build relays nothing"
    );
}

/// §2a.8: the recorded bodies carry **no** `unlock_time` and **no** `payment_id` — and
/// §2a.10: every recorded `sweep_all` carries `subaddr_indices_all: true`.
#[test]
fn the_recorded_bodies_carry_no_forbidden_field_and_every_sweep_is_indices_all() {
    let w = World::new("bodies", &[], &[]);
    let client = w.wallet();
    let node = w.node();
    let cancel = CancelToken::new();
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(DEST),
        &cancel,
    )
    .unwrap();
    drop(plan); // discarding a plan releases the §2a.11 lock, so the sweep below can build
    let sweep = SweepRequest {
        dest: DEST.to_string(),
        account_index: 0,
        priority: 2,
        below: None,
        max_fee: None,
    };
    let plan = send::build_sweep(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &sweep,
        &cancel,
    )
    .unwrap();
    assert_eq!(plan.txs.len(), 2, "sweep_all.json carries two transactions");
    drop(plan);

    for method in ["transfer_split", "sweep_all"] {
        let bodies = w.bodies(&w.wallet_log, method);
        assert!(!bodies.is_empty(), "{method} was never recorded");
        for b in &bodies {
            for forbidden in [
                "unlock_time",
                "payment_id",
                "ring_size",
                "subtract_fee_from_outputs",
            ] {
                assert!(
                    b.get(forbidden).is_none(),
                    "{method} must not send {forbidden} (§2a.8): {b}"
                );
            }
            assert_eq!(b["do_not_relay"], true, "§2a.2");
            assert_eq!(b["get_tx_metadata"], true, "§1b.4");
        }
    }
    for b in w.bodies(&w.wallet_log, "sweep_all") {
        assert_eq!(
            b["subaddr_indices_all"], true,
            "§2a.10: without it the server picks ONE subaddress at random: {b}"
        );
        assert!(
            b.get("subaddr_indices").is_none(),
            "there is no selector to disable it (§2a.10)"
        );
    }
    // `transfer_split` sends one destination and never asks for the transaction secret key.
    let t = &w.bodies(&w.wallet_log, "transfer_split")[0];
    assert_eq!(t["destinations"][0]["address"], DEST);
    assert_eq!(t["get_tx_keys"], false, "§2a.2, §2b.2");
    assert!(t.get("get_tx_key").is_none(), "split uses the plural field");
    assert_eq!(
        w.count(&w.wallet_log, "transfer"),
        0,
        "no legacy build fallback"
    );
}

// ---- §2a.4: the record, and what blocks ----

/// An entry with `relayed: false` means a send **may be in flight**: the next one exits 3.
#[test]
fn an_unrelayed_record_blocks_the_next_send_with_reason_pending_send() {
    let w = World::new("blocked", &[], &[]);
    state::set_pending_send(
        &w.ctx,
        &PendingSend {
            kind: PendingKind::Send,
            account_index: 0,
            dest: DEST.to_string(),
            built_at: crate::now_unix(),
            txs: vec![PendingTx {
                txid: BUILT_TXID.to_string(),
                amount: Piconero(1),
                fee: Piconero(2),
                relayed: false,
                relayed_at: None,
            }],
        },
    )
    .unwrap();
    let client = w.wallet();
    let node = w.node();
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("pending_send"));
    assert!(e.hint().contains("clear-pending"), "{}", e.hint());
    // Nothing was built: the refusal is at step 4, before the daemon is asked for anything.
    assert_eq!(w.count(&w.wallet_log, "transfer_split"), 0);
    // And the record is still there: a check that cleared it would disarm the interlock.
    assert!(state::pending_send(&w.ctx).unwrap().is_some());
}

#[test]
fn empty_or_whitespace_payment_ledger_blocks_send_and_sweep_until_cleared() {
    let world = World::new("empty-ledger-payments", &[], &[]);
    let client = world.wallet();
    let node = world.node();
    let cancel = CancelToken::new();
    let path = world.ctx.paths.pending_file();
    let sweep = SweepRequest {
        dest: DEST.to_string(),
        account_index: 0,
        priority: 2,
        below: None,
        max_fee: None,
    };
    assert!(state::pending_send(&world.ctx).unwrap().is_none());
    send::refuse_if_send_in_flight(&world.ctx).unwrap();

    for bytes in [b"".as_slice(), b" \t\r\n".as_slice()] {
        crate::write_private(&path, bytes).unwrap();
        let send_error = send::build(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &a_send(DEST),
            &cancel,
        )
        .unwrap_err();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let sweep_error = send::build_sweep(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &sweep,
            &cancel,
        )
        .unwrap_err();
        for error in [send_error, sweep_error] {
            assert_eq!(error.exit_code(), 3);
            assert_eq!(error.reason(), Some("pending_send"));
        }
        for method in ["transfer_split", "sweep_all", "relay_tx"] {
            assert_eq!(world.count(&world.wallet_log, method), 0, "{method}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        state::clear_pending_send(&world.ctx).unwrap();
        assert!(!path.exists());
        assert!(state::pending_send(&world.ctx).unwrap().is_none());
        send::refuse_if_send_in_flight(&world.ctx).unwrap();
    }

    // Explicit recovery restores normal preparation; neither plan is relayed.
    drop(
        send::build(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &a_send(DEST),
            &cancel,
        )
        .unwrap(),
    );
    drop(
        send::build_sweep(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &sweep,
            &cancel,
        )
        .unwrap(),
    );
    assert_eq!(world.count(&world.wallet_log, "transfer_split"), 1);
    assert_eq!(world.count(&world.wallet_log, "sweep_all"), 1);
    assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
}

/// A **fully relayed** record whose txid the wallet reports resolves itself: the send goes
/// through and the record is gone. This is what keeps `clear-pending` the manual escape hatch
/// instead of something the user learns to type after every send (§2a.4).
#[test]
fn a_fully_relayed_record_resolves_itself_and_the_send_proceeds() {
    let w = World::new("resolve", &[], &[]);
    let relayed_record = |txid: &str| PendingSend {
        kind: PendingKind::Send,
        account_index: 0,
        dest: DEST.to_string(),
        built_at: crate::now_unix(),
        txs: vec![PendingTx {
            txid: txid.to_string(),
            amount: Piconero(1),
            fee: Piconero(2),
            relayed: true,
            relayed_at: Some(crate::now_unix()),
        }],
    };
    state::set_pending_send(&w.ctx, &relayed_record(BUILT_TXID)).unwrap();
    let client = w.wallet();
    let node = w.node();
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect("a resolved record must not block");
    drop(plan);
    assert!(
        state::pending_send(&w.ctx).unwrap().is_none(),
        "the resolve step clears a fully-found record (§2a.4)"
    );
    assert!(w.count(&w.wallet_log, "get_transfer_by_txid") >= 1);
}

/// §2a.4: **a `failed`-type row counts as found.** `on_get_transfer_by_txid` searches the same
/// source that feeds both the `pending` and the `failed` lists, and a wallet that saw the
/// transaction fail is not a double-spend risk — omitting that type would leave the record in
/// place forever.
#[test]
fn a_failed_row_counts_as_found_and_the_record_clears() {
    let w = World::new("failed-row", &[], &["--scenario", "failed"]);
    state::set_pending_send(
        &w.ctx,
        &PendingSend {
            kind: PendingKind::Send,
            account_index: 0,
            dest: DEST.to_string(),
            built_at: crate::now_unix(),
            txs: vec![PendingTx {
                txid: BUILT_TXID.to_string(),
                amount: Piconero(1),
                fee: Piconero(2),
                relayed: true,
                relayed_at: Some(crate::now_unix()),
            }],
        },
    )
    .unwrap();
    // The fixture the scenario selects really is a `failed` row, or this test proves nothing.
    let client = w.wallet();
    let row = client
        .get_transfer_by_txid(BUILT_TXID, 0)
        .unwrap()
        .expect("the fixture answers");
    assert_eq!(row.kind, crate::history::TransferKind::Failed);
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect("a failed row is an answer, so the record resolves");
    drop(plan);
    assert!(state::pending_send(&w.ctx).unwrap().is_none());
}

/// §2a.4: **a positive "not found" is the only answer allowed to block**, and every other
/// failure of a resolve call leaves the record and does **not** block — otherwise the
/// interlock arms exactly when the node hiccups after a send.
#[test]
fn a_not_found_blocks_while_a_failed_resolve_call_does_not() {
    // `-8 Transaction not found.` is the positive answer: it must block.
    let w = World::new(
        "not-found",
        &[],
        &["--fail", "get_transfer_by_txid=-8:Transaction not found."],
    );
    let record = PendingSend {
        kind: PendingKind::Send,
        account_index: 0,
        dest: DEST.to_string(),
        built_at: crate::now_unix(),
        txs: vec![PendingTx {
            txid: BUILT_TXID.to_string(),
            amount: Piconero(1),
            fee: Piconero(2),
            relayed: true,
            relayed_at: Some(crate::now_unix()),
        }],
    };
    state::set_pending_send(&w.ctx, &record).unwrap();
    let e = send::build(
        &w.ctx,
        &w.wallet(),
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("pending_send"));
    assert!(state::pending_send(&w.ctx).unwrap().is_some());
    drop(w);

    // epee's `code: 0` catch-all — the ordinary answer when wallet-rpc cannot reach the
    // daemon (§1b.4) — is NOT evidence of a second spend and must not block.
    let w = World::new("resolve-broke", &[], &["--fail", "get_transfer_by_txid=0:"]);
    state::set_pending_send(&w.ctx, &record).unwrap();
    let plan = send::build(
        &w.ctx,
        &w.wallet(),
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect("a broken resolve call must not block (§2a.4)");
    drop(plan);
    assert!(
        state::pending_send(&w.ctx).unwrap().is_some(),
        "and it leaves the record in place"
    );
}

/// §2a.9: **the relayed txid must be the txid we showed.** On a mismatch the entry is NOT
/// marked relayed, the command exits 1 `txid_mismatch`, and `status` raises
/// `send_may_be_in_flight`.
#[test]
fn a_txid_mismatch_exits_one_and_leaves_the_entry_unrelayed() {
    let w = World::new("mismatch", &[], &["--scenario", "mismatch"]);
    let client = w.wallet();
    let cancel = CancelToken::new();
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &cancel,
    )
    .unwrap();
    let e = send::relay(&w.ctx, &client, plan, &|_| {}, &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    assert_eq!(e.reason(), Some("txid_mismatch"));
    // **Neither 64-character id is in the sentence a person reads, and both are in `detail`**
    // (round 4 iteration 3, §2o rule 5). The sentence also must not tell the reader to work out
    // which of two near-identical ids landed, which is what its old hint did.
    let why = e.to_string();
    for id in [
        "a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f",
        "1ae7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f",
    ] {
        assert!(!why.contains(id), "an id reached the sentence: {why}");
        assert!(
            !e.hint().contains(id),
            "an id reached the hint: {}",
            e.hint()
        );
    }
    assert!(
        !why.contains("transaction"),
        "§2o.2 says payment, not transaction: {why}"
    );
    let detail = e
        .detail()
        .expect("both ids stay in detail, for --json and -v");
    assert!(
        detail.contains("node ") && detail.contains("prepared "),
        "detail names which id came from where: {detail}"
    );
    let record = state::pending_send(&w.ctx)
        .unwrap()
        .expect("the record stays");
    assert_eq!(record.txs.len(), 1);
    assert!(
        !record.txs[0].relayed,
        "the entry must NOT be marked relayed (§2a.9)"
    );
    // And `status` says so, at severity error.
    let s = crate::status::status(&w.ctx).unwrap();
    let warning = s
        .warnings
        .iter()
        .find(|x| x.id == "send_may_be_in_flight")
        .expect("send_may_be_in_flight");
    assert_eq!(warning.severity, crate::status::Severity::Error);
}

/// §2a.2: `--max-fee` refuses **after the build and before the `--yes` check**, and the mock
/// records **zero** `relay_tx`. The refusal is `send::check_max_fee` on the returned plan, so
/// the caller still has the plan §5c's refusal payload is rendered from.
#[test]
fn a_fee_above_max_refuses_and_relays_nothing() {
    let w = World::new("max-fee", &[], &[]);
    let mut req = a_send(DEST);
    // The fixture's fee is 30_720_000 piconero.
    req.max_fee = Some(Piconero(1));
    let plan = send::build(
        &w.ctx,
        &w.wallet(),
        send::NodeGate::Local(&w.node()),
        &req,
        &CancelToken::new(),
    )
    .expect("the build itself succeeds; step 9 is the caller's check");
    let e = send::check_max_fee(&plan).unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("fee_above_max"));
    // §5c: the plan survives the refusal, so the payload can carry the BUILT numbers.
    assert_eq!(plan.total_fee, Piconero(30_720_000));
    drop(plan);
    assert_eq!(w.count(&w.wallet_log, "relay_tx"), 0);
    assert_eq!(
        w.count(&w.wallet_log, "transfer_split"),
        1,
        "it did build first"
    );

    // And `relay` re-checks it, so a plan built by other means still cannot get past it.
    let mut req = a_send(DEST);
    req.max_fee = Some(Piconero(30_720_000));
    let client = w.wallet();
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &req,
        &CancelToken::new(),
    )
    .expect("a fee exactly at the bound is allowed");
    assert_eq!(plan.total_fee, Piconero(30_720_000));
    drop(plan);
}

/// §2d: "**`send::relay` likewise never consults a breaker**: once a plan is built the §2a.11
/// `send.lock` is held and the transaction exists, so exiting 5 there would abandon a built
/// transaction rather than protect anything."
///
/// The relay must therefore go through even on a client that has never completed a request —
/// which is exactly the TUI's `Action::Relay`, built with `WalletClient::from_ctx` after
/// `Action::BuildTx` finished on a different one. The first assertion proves the breaker really
/// is armed, so this test cannot pass by the breaker being absent.
#[test]
fn an_armed_breaker_never_blocks_a_relay_even_on_a_fresh_client() {
    let w = World::new("relay-breaker", &[], &[]);
    let plan = send::build(
        &w.ctx,
        &w.wallet(),
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect("a plan");
    // §2d says a wallet probe killed at its 5 s deadline while another caller holds a long call
    // is killed "by construction", so this state is ordinary rather than exceptional.
    crate::state::note_stall(&w.ctx, crate::Daemon::Wallet).expect("arm the breaker");
    let blocked = w.wallet().get_height().unwrap_err();
    assert_eq!(
        blocked.reason(),
        Some("stalled"),
        "the breaker must really be armed, or this test proves nothing"
    );

    let fresh = w.wallet();
    let txids = send::relay(&w.ctx, &fresh, plan, &|_| {}, &CancelToken::new())
        .expect("§2d: a relay is never blocked by a breaker");
    assert_eq!(txids.len(), 1);
    // The fresh client has no Digest session yet, so its first request is answered 401 and
    // retried once (§2c) — both attempts are logged. What matters is that it got through.
    assert!(
        w.count(&w.wallet_log, "relay_tx") >= 1,
        "the relay never reached the daemon"
    );
    // And it left no record claiming a transaction nobody broadcast (§2a.4).
    assert!(crate::state::pending_send(&w.ctx).unwrap().is_none());
}

/// §2a.11: **only one build→relay sequence may be in flight.** The lock is taken before the
/// entry check and released when the plan drops, so a second build while the first plan is
/// alive is exit 3 `send_in_progress` — and a discarded plan leaves nothing to clean up.
#[test]
fn the_send_lock_is_held_by_the_plan_and_released_when_it_drops() {
    let w = World::new("send-lock", &[], &[]);
    let client = w.wallet();
    let first = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap();
    // A second build in this process would take the same flock; `flock` is per open file
    // description, so a second `open` + `LOCK_EX|LOCK_NB` conflicts exactly as another
    // process would.
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("send_in_progress"));
    assert!(e.hint().contains("status"), "{}", e.hint());
    drop(first);
    let again = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &CancelToken::new(),
    );
    assert!(again.is_ok(), "a discarded plan releases the lock (§2a.11)");
}

/// §2a.4's write order: **a two-transaction sweep observed between its first and its second
/// `relay_tx` has exactly one `relayed: true` and one `relayed: false` on disk.**
///
/// That is the state a `SIGKILL` there would leave, and it is what the ordering buys: the whole
/// record is written and fsynced before the first relay, and each entry is flipped and
/// re-fsynced *before the next call*. The relay runs on a background thread and the mock delays
/// every answer, so the window is real rather than a race; the record is read from this thread
/// while the second relay is still in flight. The version with an actual process kill is
/// `tests/e2e.sh --full`, because `fork(2)` inside a multi-threaded test binary is not safe to
/// follow with arbitrary Rust code.
#[test]
fn a_sweep_observed_between_two_relays_has_one_relayed_and_one_not() {
    let w = World::new("mid-sweep", &[], &["--delay-ms", "1200"]);
    let ctx = w.ctx.clone();
    let log = w.wallet_log.clone();
    let relaying = std::thread::spawn(move || {
        let client = crate::rpc::WalletClient::from_ctx(&ctx).unwrap();
        let node = crate::rpc::NodeClient::from_ctx(&ctx).unwrap();
        let cancel = CancelToken::new();
        let sweep = SweepRequest {
            dest: DEST.to_string(),
            account_index: 0,
            priority: 2,
            below: None,
            max_fee: None,
        };
        send::build_sweep(&ctx, &client, send::NodeGate::Local(&node), &sweep, &cancel)
            .and_then(|plan| send::relay(&ctx, &client, plan, &|_| {}, &cancel))
    });
    // Poll the RECORD, not the request log: the mock logs a request only after its delay, so by
    // the time the log shows two relays the second has already been answered and marked.
    let mut seen: Option<PendingSend> = None;
    for _ in 0..1200 {
        if let Ok(Some(r)) = state::pending_send(&w.ctx) {
            let relayed = r.txs.iter().filter(|t| t.relayed).count();
            if r.txs.len() == 2 && relayed == 1 {
                seen = Some(r);
                break;
            }
            if relayed == 2 {
                panic!("the window was missed: both relays had already been marked");
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let record = seen.expect("the record must pass through exactly one relayed: true (§2a.4)");
    assert_eq!(record.txs.len(), 2);
    assert_eq!(record.kind, PendingKind::Sweep);
    assert!(
        record.txs[0].relayed && !record.txs[1].relayed,
        "the entries are flipped in relay order: {record:?}"
    );
    assert!(record.txs[0].relayed_at.is_some());
    assert!(record.txs[1].relayed_at.is_none());
    let _ = &log;
    // Let the send finish, so the temp dir can be removed and the whole sequence is proved to
    // complete: two relays, one store, and a record that clears itself (§2a.4).
    let txids = relaying
        .join()
        .expect("no panic")
        .expect("the sweep succeeds");
    assert_eq!(txids.len(), 2);
    assert_eq!(w.count(&w.wallet_log, "relay_tx"), 2);
    assert_eq!(w.count(&w.wallet_log, "store"), 1);
}

/// §2a.4's partial sweep: a `relay_tx` that fails at entry *k* of *N* aborts, leaves the
/// record and exits **16 `tx_rejected`** naming what did go out.
#[test]
fn a_refused_relay_leaves_the_record_and_names_what_was_broadcast() {
    let w = World::new(
        "relay-refused",
        &[],
        &["--fail", "relay_tx=-4:Transaction was rejected"],
    );
    let client = w.wallet();
    let cancel = CancelToken::new();
    let plan = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&w.node()),
        &a_send(DEST),
        &cancel,
    )
    .unwrap();
    let e = send::relay(&w.ctx, &client, plan, &|_| {}, &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 16);
    assert_eq!(e.reason(), Some("relay_refused"));
    // X2: the sentence says it in words, and `broadcast` is gone from it.
    assert!(
        e.to_string()
            .contains("none of the 1 payments has gone out"),
        "{e}"
    );
    assert!(!e.to_string().contains("broadcast"), "{e}");
    assert!(e.hint().contains("clear-pending"), "{}", e.hint());
    let record = state::pending_send(&w.ctx).unwrap().unwrap();
    assert!(!record.txs[0].relayed);
}

// ---- §2a.1's refusal order ----

/// The order is what decides the exit code, so each step is asserted where the next one would
/// have answered differently (§2a.1).
#[test]
fn the_refusal_order_decides_the_exit_code() {
    let w = World::new("order", &[], &[]);
    let client = w.wallet();
    let node = w.node();
    let cancel = CancelToken::new();

    // 1. A zero amount is exit 2 `bad_amount`, before anything else.
    let mut req = a_send(DEST);
    req.amount = Piconero(0);
    let e = send::build(&w.ctx, &client, send::NodeGate::Local(&node), &req, &cancel).unwrap_err();
    assert_eq!((e.exit_code(), e.reason()), (2, Some("bad_amount")));

    // 2. A structurally impossible address is exit 15, before the lock and before any RPC.
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send("not-an-address"),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 15);
    assert_eq!(w.count(&w.wallet_log, "validate_address"), 0);

    // 6. A wrong-nettype recipient is exit 15, with the nettype named.
    // A real stagenet standard address, from `tests/fixtures/address-forms.json`.
    const STAGENET: &str = "53teqCAESLxeJ1REzGMAat1ZeHvuajvDiXqboEocPaDRRmqWoVPzy46GLo866qRFjbNhfkNckyhST3WEvBviDwpUDd7DSzB";
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(STAGENET),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 15);
    assert!(e.to_string().contains("stagenet"), "{e}");
    assert_eq!(
        w.count(&w.wallet_log, "transfer_split"),
        0,
        "nothing was built"
    );

    // 5a. `@label`: zero matches is exit 6, several is exit 7, one resolves to the ADDRESS.
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send("@nothing"),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 6, "reason={:?} msg={e}", e.reason());
    // `get_address_book.json` carries two entries described "Exchange" and "exchange", which
    // is what makes the case-insensitive match ambiguous (§4).
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send("@exchange"),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 7);
    // One match resolves, and **the resolved address — never the label — is what reaches the
    // daemon** (§4). No `validate_address` fixture answers for the address book's entries, so
    // the build then fails at step 6; the recorded request is the evidence that matters here.
    let e = send::build(
        &w.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send("@savings"),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 15, "{e}");
    let asked = w.bodies(&w.wallet_log, "validate_address");
    let last = asked.last().expect("validate_address was called");
    assert_eq!(
        last["address"].as_str().unwrap(),
        "44Kbx4sJ7JDRDV5aAhLJzQCjDz2ViLRduE3ijDZu3osWKBjMGkV1XPk4pfDUMqt1Aiezvephdqm6YD19GKFD9ZcXVUTp6BW",
        "the label must never reach the wire (§4)"
    );
    // And the two §2a.6 flags are always these.
    assert_eq!(last["any_net_type"], true);
    assert_eq!(last["allow_openalias"], false);

    // 7. A node that is not synchronized is exit 10 — and nothing is built.
    let before = w.count(&w.wallet_log, "transfer_split");
    let syncing = World::new("order-syncing", &["--scenario", "syncing"], &[]);
    let e = send::build(
        &syncing.ctx,
        &syncing.wallet(),
        send::NodeGate::Local(&syncing.node()),
        &a_send(DEST),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 10);
    assert_eq!(syncing.count(&syncing.wallet_log, "transfer_split"), 0);
    assert_eq!(w.count(&w.wallet_log, "transfer_split"), before);
}

/// §2a.1: **a valid address with the wallet daemon down is exit 4, not the no-`--yes` 3** —
/// and a bad address is still 15, because steps 1–4 are local and run first.
#[test]
fn with_the_wallet_daemon_down_a_valid_address_is_exit_four() {
    let dir = std::env::temp_dir().join(format!("fmonero-down-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).unwrap();
    // A port nothing is listening on. No login file either, which is the §4b GREEN row.
    ctx.settings.wallet.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.port = 1;
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.timeouts.wallet = Duration::from_secs(2);
    // `from_ctx` must SUCCEED with no login file, or the constructor would decide the exit
    // code from outside the refusal order (§2a.1, §2c).
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("§2c: an absent login file is Ok");
    let e = send::build(
        &ctx,
        &client,
        send::NodeGate::Unreachable,
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    assert_eq!(e.reason(), Some("unreachable"));
    // And a bad address still loses first.
    let e = send::build(
        &ctx,
        &client,
        send::NodeGate::Unreachable,
        &a_send("not-an-address"),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 15);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- §2c: Digest -------------------------------------------------------------

/// §2c's three assertions, against the mock's own two-header epee challenge: the client picks
/// the **`MD5`** challenge and never `MD5-sess`; the second request on one nonce carries
/// `nc=00000002`; and after the mock rotates the nonce the next request carries `nc=00000001`
/// again.
///
/// It needs keep-alive: an accept-respond-close loop would make the client open a fresh
/// connection per request, correctly reset `nc` every time, and the one non-obvious property
/// of `digest.rs` would go untested.
#[test]
fn the_nc_counter_advances_within_a_nonce_and_resets_when_it_rotates() {
    let w = World::new("digest", &[], &["--rotate-nonce-after", "2"]);
    let client = w.wallet();
    // Four authenticated calls on one client, i.e. one connection.
    for _ in 0..4 {
        client.get_version().expect("the handshake must succeed");
    }
    let log = read_log(&w.wallet_log);
    let ncs: Vec<Option<&str>> = log.iter().map(|l| l["nc"].as_str()).collect();
    let statuses: Vec<u64> = log
        .iter()
        .map(|l| l["status"].as_u64().unwrap_or(0))
        .collect();
    // The first request is unauthenticated (no nonce yet) and is answered 401 once.
    assert_eq!(ncs[0], None, "{ncs:?}");
    assert_eq!(statuses[0], 401, "{statuses:?}");
    let authed: Vec<&str> = ncs.iter().flatten().copied().collect();
    assert_eq!(
        authed.first(),
        Some(&"00000001"),
        "the first request on a NEW nonce is nc=00000001 (§1b.7): {ncs:?}"
    );
    assert!(
        authed.contains(&"00000002"),
        "the second request on one nonce is nc=00000002 (§2c): {ncs:?}"
    );
    // After the rotation the client is challenged again and starts over at 1.
    assert!(
        authed.iter().filter(|n| **n == "00000001").count() >= 2,
        "a rotated nonce resets nc (§1b.7): {ncs:?}"
    );
    // A `stale` 401 was answered and retried, never given up on.
    assert!(
        statuses.iter().filter(|s| **s == 401).count() >= 2,
        "{statuses:?}"
    );
    assert_eq!(statuses.last(), Some(&200), "{statuses:?}");
    // The wire says we authenticated, which is what §5's `wallet.authenticated` reports.
    assert_eq!(client.authenticated(), Some(true));
}

/// §2c: with no login file the client sends unauthenticated requests, the daemon answers 401
/// twice, and the command exits **4 `auth_refused`** — a different answer from "nothing is
/// listening", which is 4 `unreachable`.
#[test]
fn a_missing_login_file_is_auth_refused_and_not_unreachable() {
    let w = World::new("no-login", &[], &[]);
    std::fs::remove_file(w.ctx.paths.wallet_login_file(w.ctx.settings.wallet.port)).unwrap();
    let client = crate::rpc::WalletClient::from_ctx(&w.ctx).expect("§2c: still Ok");
    let e = client.get_height().unwrap_err();
    assert_eq!(e.exit_code(), 4);
    assert_eq!(e.reason(), Some("auth_refused"));
    assert_eq!(client.authenticated(), None, "nothing completed");
}

// ---- §1b.2, §1b.3: the node ---------------------------------------------------

/// §1b.2: `sync_info` answering `-32601` **is** the restricted-RPC probe, and a gated *legacy*
/// endpoint answers HTTP 404 instead — two transports, two shapes, one verdict.
#[test]
fn the_restricted_node_is_detected_on_both_transports() {
    let w = World::new("restricted", &["--restricted"], &[]);
    let n = w.node();
    assert!(n.is_restricted().unwrap(), "-32601 from sync_info (§1b.2)");
    let e = n.get_peer_list(true).unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert_eq!(
        e.reason(),
        Some("restricted"),
        "a gated legacy endpoint is 404, not -32601 (§1b.2)"
    );
    // `get_info` stays open on a restricted node, which is why it is the reachability probe.
    assert!(n.get_info().is_ok());
}

/// §1b.3 and §5: a **restricted** node poisons part of `get_info`, and every poisoned field
/// becomes `null` — never a 1970 uptime, never 16 exabytes free, never a false "no peers".
#[test]
fn a_restricted_nodes_poisoned_fields_render_as_null() {
    let w = World::new("poisoned", &["--scenario", "restricted"], &[]);
    let s = crate::status::status(&w.ctx).unwrap();
    assert_eq!(s.node.restricted, Some(true));
    assert_eq!(s.node.reachable, Some(true));
    for (what, value) in [
        ("uptime_sec", s.node.uptime_sec.is_none()),
        ("free_space", s.node.free_space.is_none()),
        ("version", s.node.version.is_none()),
        ("update_available", s.node.update_available.is_none()),
        ("alt_blocks_count", s.node.alt_blocks_count.is_none()),
        (
            "outgoing_connections_count",
            s.node.outgoing_connections_count.is_none(),
        ),
        (
            "incoming_connections_count",
            s.node.incoming_connections_count.is_none(),
        ),
        ("white_peerlist_size", s.node.white_peerlist_size.is_none()),
        ("grey_peerlist_size", s.node.grey_peerlist_size.is_none()),
    ] {
        assert!(value, "node.{what} must be null on a restricted node (§5)");
    }
    // These are real on a restricted node and stay populated.
    assert!(s.node.height.is_some());
    assert!(s.node.synchronized.is_some());
    assert!(s.node.nettype.is_some());
    // `database_size` is rounded, not poisoned, and carries a sibling flag saying so.
    assert!(s.node.database_size.is_some());
    assert!(s.node.database_size_rounded);
    // `tx_pool_size` is a lower bound, and the warning text says so.
    assert!(s.node.tx_pool_size.is_some());
    let w2 = s
        .warnings
        .iter()
        .find(|x| x.id == "node_restricted")
        .expect("node_restricted");
    assert!(w2.text.contains("is a minimum"), "{}", w2.text);
    // And `node_no_peers` must NOT fire: a restricted node reports 0 by construction (§5).
    assert!(!s.warnings.iter().any(|x| x.id == "node_no_peers"));
}

/// §1b.3's `"BUSY"` degradation: `get_last_block_header` is the one call `CHECK_CORE_READY()`
/// gates, it answers an **all-zero header**, and `top_block_age_sec` must be `null` — not a
/// block 56 years old — with `status` still exiting 0.
#[test]
fn a_busy_block_header_renders_null_and_status_still_succeeds() {
    let w = World::new("busy", &["--scenario", "syncing"], &[]);
    let s = crate::status::status(&w.ctx).expect("status still succeeds");
    assert!(s.ok);
    assert_eq!(s.node.reachable, Some(true));
    assert_eq!(
        s.node.top_block_age_sec, None,
        "an all-zero BUSY header must not become an age (§1b.3)"
    );
    assert_eq!(s.node.synchronized, Some(false));
    assert!(s.warnings.iter().any(|x| x.id == "node_syncing"));
    // The client's own answer is an error, not a zeroed header.
    let e = w.node().get_last_block_header().unwrap_err();
    assert_eq!(e.exit_code(), 5);
    // v5.6, §2o: the sentence is plain and the wire word is the `detail`.
    assert!(e.to_string().contains("still catching up"), "{e}");
    assert!(e.detail().unwrap_or_default().contains("BUSY"), "{e:?}");
}

/// §5's hot path with a wallet open: three calls closed, five open — and the whole document
/// is populated from the fixtures.
#[test]
fn the_status_hot_path_is_three_calls_closed_and_five_open() {
    let w = World::new("hot", &[], &[]);
    let s = crate::status::status(&w.ctx).unwrap();
    assert!(s.wallet.reachable && s.wallet.open);
    assert_eq!(s.wallet.balance_xmr.as_deref(), Some("1.234567891234"));
    assert_eq!(s.wallet.balance_pico, Some(Piconero(1_234_567_891_234)));
    // Every GENERATED subaddress (three in get_address.json), not the one funded row (§5).
    assert_eq!(s.wallet.subaddress_count, Some(3));
    assert_eq!(s.wallet.blocks_behind, Some(0));
    assert!(s.node.height.is_some() && s.node.top_block_age_sec.is_some());
    // The hot path is exactly the five §5 names, plus the cold set once.
    let wallet_calls: Vec<String> = read_log(&w.wallet_log)
        .iter()
        .filter(|l| l["status"] == 200)
        .map(|l| l["method"].as_str().unwrap_or_default().to_string())
        .collect();
    for hot in ["get_height", "get_balance", "get_address"] {
        assert!(wallet_calls.contains(&hot.to_string()), "{wallet_calls:?}");
    }
    for never in [
        "refresh",
        "open_wallet",
        "close_wallet",
        "transfer_split",
        "relay_tx",
    ] {
        assert!(
            !wallet_calls.contains(&never.to_string()),
            "§2e: status calls no wallet mutator, found {never}"
        );
    }
    let node_calls: Vec<String> = read_log(&w.node_log)
        .iter()
        .map(|l| l["method"].as_str().unwrap_or_default().to_string())
        .collect();
    for never in ["sync_info", "get_connections", "get_peer_list"] {
        assert!(
            !node_calls.contains(&never.to_string()),
            "§5: status never calls {never}"
        );
    }
    // A second invocation serves the cold set from state: no second `hard_fork_info`.
    let before = w.count(&w.node_log, "hard_fork_info");
    assert_eq!(before, 1);
    crate::status::status(&w.ctx).unwrap();
    assert_eq!(
        w.count(&w.node_log, "hard_fork_info"),
        1,
        "the cold set is cached for 60 s (§5)"
    );
    // The rate needs two samples; the second invocation is the second sample.
    let s = crate::status::status(&w.ctx).unwrap();
    assert!(s.staleness.node_extras_age_sec.is_some());
    assert!(s.node.net.total_bytes_in.is_some());
}

/// §2d: a call killed at its deadline arms a 60 s breaker, the next command that needs that
/// daemon exits 5 `stalled` immediately, and `--force` tries anyway — while `status` neither
/// arms one nor is blocked by one.
#[test]
fn a_deadline_kill_arms_a_breaker_that_force_overrides_and_status_ignores() {
    let w = World::new("breaker", &[], &["--delay-ms", "3000"]);
    let mut ctx = w.ctx.clone();
    ctx.timeouts.wallet = Duration::from_millis(300);
    ctx.timeouts.open = Duration::from_millis(300);
    ctx.timeouts.transfer = Duration::from_millis(300);
    ctx.timeouts.refresh = Duration::from_millis(300);
    ctx.timeouts.relay = Duration::from_millis(300);

    let client = crate::rpc::WalletClient::from_ctx(&ctx).unwrap();
    let e = client.get_height().unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert_eq!(e.reason(), Some("killed_at_deadline"));
    let left = state::stall_remaining(&ctx, state::Daemon::Wallet)
        .unwrap()
        .expect("the breaker is armed");
    assert!(left > 0 && left <= 60, "{left}");

    // A fresh client now refuses immediately, with a different reason.
    let blocked = crate::rpc::WalletClient::from_ctx(&ctx).unwrap();
    let started = std::time::Instant::now();
    let e = blocked.get_height().unwrap_err();
    assert_eq!(e.reason(), Some("stalled"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the point of a breaker is not waiting again"
    );

    // `--force` ignores it for this invocation and tries the daemon anyway (§4).
    let mut forced = ctx.clone();
    forced.force = true;
    let e = crate::rpc::WalletClient::from_ctx(&forced)
        .unwrap()
        .get_height()
        .unwrap_err();
    assert_eq!(
        e.reason(),
        Some("killed_at_deadline"),
        "--force attempts the call (§2d)"
    );

    // And `status` is never blocked by a breaker: it reports it.
    let s = crate::status::status(&ctx).unwrap();
    assert!(s.ok);
    assert!(s.warnings.iter().any(|x| x.id == "wallet_stalled"));
}

/// §2d: a wallet call killed at its deadline while the wallet is **more than 100 blocks
/// behind** is `wallet_syncing`, **not** a stall — `monero-wallet-rpc` serves RPC on one
/// thread and deliberately blocks it while it catches up, so a breaker that fired here would
/// fire on correct behaviour.
#[test]
fn a_timeout_while_the_wallet_is_far_behind_arms_no_breaker() {
    let w = World::new("syncing-wallet", &[], &["--delay-ms", "3000"]);
    // What `status` observed last: the wallet is a long way behind.
    state::with_state(&w.ctx, |s| s.wallet_behind = Some(5_000)).unwrap();
    let mut ctx = w.ctx.clone();
    ctx.timeouts.wallet = Duration::from_millis(300);
    let e = crate::rpc::WalletClient::from_ctx(&ctx)
        .unwrap()
        .get_height()
        .unwrap_err();
    assert_eq!(e.reason(), Some("killed_at_deadline"));
    assert_eq!(
        state::stall_remaining(&ctx, state::Daemon::Wallet).unwrap(),
        None,
        "§2d: that is wallet_syncing, not a stall"
    );
    // Within 100 blocks it does arm.
    state::with_state(&ctx, |s| s.wallet_behind = Some(3)).unwrap();
    let _ = crate::rpc::WalletClient::from_ctx(&ctx)
        .unwrap()
        .get_height();
    assert!(state::stall_remaining(&ctx, state::Daemon::Wallet)
        .unwrap()
        .is_some());
}

// ---- §1b.4: the history top-up ------------------------------------------------

/// §1b.4: the top-up sends `filter_by_height: true` **together with** `min_height` — without
/// the flag the bound is silently ignored and the daemon returns the whole history — and the
/// six-variant `TransferKind` survives the `block` (coinbase) row.
#[test]
fn the_history_top_up_sends_the_height_filter_and_reads_a_coinbase_row() {
    let w = World::new("history", &[], &[]);
    let client = w.wallet();
    let mut q = crate::history::HistoryQuery {
        account_index: 0,
        limit: 100,
        kinds: crate::history::TransferKind::ALL.to_vec(),
        ..Default::default()
    };
    let rows = crate::history::query(&w.ctx, Some(&client), &mut q).expect("the top-up");
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .any(|t| t.kind == crate::history::TransferKind::Block),
        "a coinbase row must deserialize (§1b.4)"
    );
    // The wallet's own height is written back, never a caller's (§5b).
    assert_eq!(q.wallet_height, Some(3_541_122));
    assert!(q.adjusted_now.is_some());
    let body = &w.bodies(&w.wallet_log, "get_transfers")[0];
    assert_eq!(body["filter_by_height"], true, "§1b.4: {body}");
    assert!(body["min_height"].is_u64());
    assert!(body.get("max_height").is_none(), "left unset (§1b.4)");
    // `pool` is sent only because this query asked for pool rows.
    assert_eq!(body["pool"], true);

    // A query that does NOT ask for pool rows must not send `pool`, because that branch alone
    // is a daemon round trip (§1b.4).
    let mut q = crate::history::HistoryQuery {
        account_index: 0,
        limit: 5,
        kinds: vec![crate::history::TransferKind::In],
        ..Default::default()
    };
    crate::history::query(&w.ctx, Some(&client), &mut q).unwrap();
    let body = w.bodies(&w.wallet_log, "get_transfers").pop().unwrap();
    assert_eq!(body["pool"], false, "{body}");

    // `--cached` issues NOTHING at all, and then both derived fields are null (§4.1, §2l (d)).
    let before = read_log(&w.wallet_log).len();
    let mut q = crate::history::HistoryQuery {
        account_index: 0,
        limit: 100,
        kinds: vec![
            crate::history::TransferKind::In,
            crate::history::TransferKind::Out,
        ],
        cached: true,
        ..Default::default()
    };
    let cached = crate::history::query(&w.ctx, None, &mut q).unwrap();
    assert_eq!(read_log(&w.wallet_log).len(), before, "zero RPC (§4.1)");
    assert_eq!(q.wallet_height, None);
    assert!(!cached.is_empty(), "the snapshot served the confirmed rows");
    for t in &cached {
        assert_eq!(crate::history::confirmations(t, q.wallet_height), None);
        assert_eq!(
            crate::history::locked(t, q.wallet_height, q.adjusted_now),
            None
        );
        // Only confirmed in/out/block rows are cached (§5b).
        assert!(matches!(
            t.kind,
            crate::history::TransferKind::In
                | crate::history::TransferKind::Out
                | crate::history::TransferKind::Block
        ));
    }
    // The header is readable and the high water is a CONFIRMED height, never the tip (§5b).
    let h = crate::history::header(&w.ctx).unwrap().expect("a header");
    assert!(h.magic_ok);
    assert_eq!(h.format_version, crate::history::CACHE_FORMAT_VERSION);
    assert!(h.high_water > 0 && h.high_water <= 3_541_122 - crate::history::SPENDABLE_AGE);
}

/// §4.1: a `get_transfers` that failed **because of the pool branch** is retried exactly once
/// without `pool`, and the caller renders `pool_unavailable: true` — a read-only command does
/// not fail because the node is down.
#[test]
fn a_pool_failure_is_retried_once_without_pool() {
    // epee's `code: 0` is what `update_pool_state` throwing produces (§1b.4). The mock fails
    // the method unconditionally, so the retry fails too — and the ERROR is then returned,
    // which is the honest outcome when even the poolless query cannot be answered.
    let w = World::new("pool-fail", &[], &["--fail", "get_transfers=0:"]);
    let client = w.wallet();
    let mut q = crate::history::HistoryQuery {
        account_index: 0,
        limit: 5,
        kinds: vec![crate::history::TransferKind::Pool],
        ..Default::default()
    };
    let e = crate::history::query(&w.ctx, Some(&client), &mut q).unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert!(e.to_string().contains("cannot reach the node"), "{e}");
    let bodies = w.bodies(&w.wallet_log, "get_transfers");
    assert_eq!(bodies.len(), 2, "exactly one retry (§4.1)");
    assert_eq!(bodies[0]["pool"], true);
    assert_eq!(bodies[1]["pool"], false, "the retry drops pool");
}

// ---- §4.2: the `wallet open` sequence ----------------------------------------

/// §4.2: `open_wallet`, **then exactly one** `set_daemon` carrying the node credential, then
/// `auto_refresh`, then the recorded name and nettype. `set_daemon` cannot come first —
/// `on_set_daemon` answers `-13 NOT_OPEN` with no wallet loaded — and without it every
/// wallet→daemon call against fmonero's own authenticated node gets a 401.
#[test]
fn opening_a_wallet_installs_the_node_credential_in_that_order() {
    let w = World::new("open", &[], &[]);
    // fmonero's own node unit has an rpc-login, so the credential must travel.
    std::fs::write(
        w.ctx.paths.monerod_conf(),
        b"data-dir=/x\nrpc-login=fmonero:0123456789abcdef0123456789abcdef\n",
    )
    .unwrap();
    let client = w.wallet();
    let opened = crate::wallet_session::open(
        &w.ctx,
        &client,
        "main",
        "hunter2".to_string(),
        &CancelToken::new(),
    )
    .expect("the open sequence");
    assert_eq!(opened.name, "main");
    assert_eq!(opened.net, crate::address::NetType::Mainnet);
    let order: Vec<String> = read_log(&w.wallet_log)
        .iter()
        .filter(|l| l["status"] == 200)
        .map(|l| l["method"].as_str().unwrap_or_default().to_string())
        .filter(|m| ["open_wallet", "set_daemon", "auto_refresh"].contains(&m.as_str()))
        .collect();
    assert_eq!(
        order,
        vec!["open_wallet", "set_daemon", "auto_refresh"],
        "the order is not free (§4.2)"
    );
    let body = &w.bodies(&w.wallet_log, "set_daemon")[0];
    assert_eq!(body["username"], "fmonero");
    assert_eq!(body["trusted"], true);
    assert!(body["address"].as_str().unwrap().starts_with("127.0.0.1:"));
    let refresh = &w.bodies(&w.wallet_log, "auto_refresh")[0];
    assert_eq!(refresh["enable"], true);
    assert_eq!(
        refresh["period"], 20,
        "the upstream default, explicit (§2d)"
    );
    // The name and the wallet's OWN nettype are recorded (§2a.6).
    assert_eq!(
        state::last_open(&w.ctx).unwrap(),
        Some(("main".to_string(), crate::address::NetType::Mainnet))
    );
    // And the password never reaches the request log in any other field.
    let log = std::fs::read_to_string(&w.wallet_log).unwrap();
    assert_eq!(
        log.matches("hunter2").count(),
        1,
        "only open_wallet's own password field carries it"
    );

    // With no `rpc-login=` at all (the §2i system_unit layout) the two fields are OMITTED.
    let w2 = World::new("open-nologin", &[], &[]);
    crate::wallet_session::open(
        &w2.ctx,
        &w2.wallet(),
        "main",
        "x".to_string(),
        &CancelToken::new(),
    )
    .unwrap();
    let body = &w2.bodies(&w2.wallet_log, "set_daemon")[0];
    assert!(body.get("username").is_none(), "{body}");
    assert!(body.get("password").is_none(), "{body}");
}

/// §4.2: a `-38 NO_DAEMON_CONNECTION` from `set_daemon` does **not** fail the open — the
/// wallet is open, it simply has no daemon yet.
#[test]
fn a_daemonless_set_daemon_does_not_fail_the_open() {
    let w = World::new("open-38", &[], &["--fail", "set_daemon=-38:no connection"]);
    let opened = crate::wallet_session::open(
        &w.ctx,
        &w.wallet(),
        "main",
        "x".to_string(),
        &CancelToken::new(),
    )
    .expect("the wallet is open (§4.2)");
    assert_eq!(opened.name, "main");
    assert_eq!(state::last_open(&w.ctx).unwrap().unwrap().0, "main");
}

// ---- §2m: remote node mode ---------------------------------------------------

/// Put this world in remote mode. The address is deliberately one **nothing here listens on**:
/// in remote mode fmonero must never dial the node itself (§2c), so a test that pointed
/// `node.remote` at the node mock could not tell a correct implementation from one that does.
fn go_remote(ctx: &mut Context, trusted: bool) {
    ctx.settings.node.remote = "node.example.com:18089".to_string();
    ctx.settings.node.remote_trusted = trusted;
    ctx.settings.node.mode = crate::settings::NodeMode::Remote;
}

/// §2c's own enforceable form of "fmonero never speaks to a remote node": there is no code path
/// in which its `ureq` agent is handed a public address, because the constructor refuses.
#[test]
fn no_node_client_can_be_constructed_in_remote_mode() {
    let w = World::new("remote-noclient", &[], &[]);
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    let e = crate::rpc::NodeClient::from_ctx(&ctx)
        .err()
        .expect("refused");
    assert_eq!(e.exit_code(), 4);
    assert_eq!(e.reason(), Some("remote_node"));
    assert_eq!(e.hint(), "fmonero node local --yes");
    // And the node mock heard nothing at all.
    assert_eq!(read_log(&w.node_log).len(), 0);
}

/// §2m.3: `wallet open` in remote mode issues exactly one `set_daemon` whose body has the
/// remote address, `trusted: false`, `ssl_support: "autodetect"` and **no `username` and no
/// `password` keys at all** — even though `monerod.conf` here carries an `rpc-login`, because
/// fmonero's own credential means nothing to a stranger's node.
///
/// On the pre-v5.2 code this fails three times over: the address was `node.url`'s host:port,
/// `trusted` was hard-coded `true`, and the credential travelled.
#[test]
fn opening_a_wallet_in_remote_mode_sends_the_remote_address_and_no_credential() {
    let w = World::new("remote-open", &[], &[]);
    std::fs::write(
        w.ctx.paths.monerod_conf(),
        b"data-dir=/x\nrpc-login=fmonero:0123456789abcdef0123456789abcdef\n",
    )
    .unwrap();
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("wallet client");
    crate::wallet_session::open(&ctx, &client, "main", "x".to_string(), &CancelToken::new())
        .expect("the open sequence");
    let body = &w.bodies(&w.wallet_log, "set_daemon")[0];
    assert_eq!(body["address"], "node.example.com:18089");
    assert_eq!(body["trusted"], false);
    assert_eq!(body["ssl_support"], "autodetect");
    assert!(body.get("username").is_none(), "{body}");
    assert!(body.get("password").is_none(), "{body}");
    // The credential is not merely absent from the body: it never entered the request log.
    let log = std::fs::read_to_string(&w.wallet_log).unwrap();
    assert!(
        !log.contains("0123456789abcdef"),
        "the node credential travelled"
    );
    // The node mock is still untouched: nothing in the open path speaks to a node.
    assert_eq!(read_log(&w.node_log).len(), 0);
}

/// §2m.3: in remote mode a `-38` from `set_daemon` is **not** swallowed. It means the address
/// did not parse (`set_daemon` opens no socket, §1b.9 item 1), and the address passed §2m.2's
/// grammar minutes ago — so reporting a successful `wallet open` for a wallet whose daemon was
/// rejected is the silence §2j forbids.
///
/// The local-mode counterpart, `a_daemonless_set_daemon_does_not_fail_the_open`, asserts the
/// opposite on the same code path: this pair is the whole of §5b.1's amendment #10.
#[test]
fn a_rejected_remote_address_fails_the_open_where_a_local_one_does_not() {
    let w = World::new(
        "remote-open-38",
        &[],
        &["--fail", "set_daemon=-38:Unable to set daemon"],
    );
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("wallet client");
    let e =
        crate::wallet_session::open(&ctx, &client, "main", "x".to_string(), &CancelToken::new())
            .expect_err("remote mode does not swallow it (§2m.3)");
    assert_eq!(e.exit_code(), 4);
    assert_eq!(e.reason(), Some("remote_address_rejected"));
    assert!(
        e.to_string()
            .contains("Monero refused the configured wallet connection."),
        "{e}"
    );
    assert!(
        !e.to_string().contains("Unable to set daemon"),
        "protected peer text must not be echoed"
    );
    assert_eq!(
        e.hint(),
        "fmonero settings list",
        "transport refusal directs the user to its complete policy"
    );
    // The wallet IS open on the daemon — the refusal is about the daemon it was given — so the
    // recorded name is not written, because `install_daemon_and_record` never reached it.
    assert!(state::last_open(&w.ctx).unwrap().is_none());
}

/// §4.2 and §2m.3, v5.3: `set_daemon` runs under `timeouts.refresh_sec`, not under the 5 s
/// probe deadline.
///
/// "`set_daemon` opens no socket" (§1b.9 item 1) is true of the call and false of its latency:
/// `wallet2::set_daemon` takes `m_daemon_rpc_mutex` first, and in remote mode the auto-refresh
/// a freshly created wallet starts holds that mutex across internet round trips. The very
/// first `wallet create` against a real public node therefore exited **5 `killed_at_deadline`**
/// for a wallet that was already on disk.
///
/// The mock's `--delay-ms` is the same latency with none of the internet: the probe deadline is
/// set **below** it and the refresh deadline above it, so the call succeeds iff it is the
/// refresh deadline that governs. On the pre-v5.3 `Deadline::Probe` it is exit 5.
#[test]
fn set_daemon_is_governed_by_the_refresh_deadline_not_the_probe_deadline() {
    let w = World::new("set-daemon-slow", &[], &["--delay-ms", "1500"]);
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    // A probe deadline the delayed answer cannot meet, and a refresh deadline it can.
    ctx.timeouts.wallet = Duration::from_millis(600);
    ctx.timeouts.refresh = Duration::from_secs(20);
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("wallet client");
    let target = crate::units::daemon_target(&ctx);
    client
        .set_daemon(&target.address, target.trusted, None, target.ssl)
        .expect("a set_daemon behind the daemon rpc mutex must not be killed at 5 s");
    let answered = read_log(&w.wallet_log)
        .iter()
        .filter(|l| l["method"] == "set_daemon" && l["status"] == 200)
        .count();
    assert_eq!(answered, 1, "exactly one set_daemon was answered");

    // And the probe deadline still governs a probe, so this test is not just "every deadline is
    // long now": `get_height` under the same 600 ms is killed.
    let e = client
        .get_height()
        .expect_err("the probe deadline still bites");
    assert_eq!(e.exit_code(), 5, "{e}");
    assert_eq!(e.reason(), Some("killed_at_deadline"));
}

/// §2m.3's send gate: **one bounded `refresh` before the `transfer_split`**, and nothing else changes.
///
/// On the pre-v5.2 code the gate was `get_info.synchronized` against a local node, which in
/// remote mode does not exist: the send refused with exit 4 `unreachable` and no `refresh` was
/// ever issued.
#[test]
fn the_remote_send_gate_is_one_refresh_before_the_build() {
    let w = World::new("remote-gate", &[], &[]);
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("wallet client");
    let plan = send::build(
        &ctx,
        &client,
        send::NodeGate::Remote,
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect("the build proceeds once the refresh answers");
    assert_eq!(plan.txs.len(), 1);
    // The ORDER is the assertion: a refresh after the build would prove nothing about the tip
    // the transaction was built against.
    let order: Vec<String> = read_log(&w.wallet_log)
        .iter()
        .map(|l| l["method"].as_str().unwrap_or_default().to_string())
        .filter(|m| m == "refresh" || m == "transfer_split")
        .collect();
    assert_eq!(order, vec!["refresh", "transfer_split"], "{order:?}");
    assert_eq!(
        w.count(&w.wallet_log, "refresh"),
        1,
        "ONE refresh, not a poll"
    );
    // And it was `start_height: 0` — the whole point is "the wallet is at that node's tip".
    assert_eq!(w.bodies(&w.wallet_log, "refresh")[0]["start_height"], 0);
    // `wallet_session::refresh` recorded it, which is what §5's `wallet.last_refresh_at` reads.
    assert!(state::load(&ctx).unwrap().last_refresh.is_some());
    // No node call, in either direction.
    assert_eq!(read_log(&w.node_log).len(), 0);
}

/// §2m.3: a `-38` (or a transport failure) from that refresh is exit **4 `unreachable`**, with
/// **zero** `transfer_split` calls — the remote daemon did not answer, so there is no tip to build on.
#[test]
fn a_refresh_that_cannot_reach_the_remote_node_refuses_the_send() {
    let w = World::new(
        "remote-gate-38",
        &[],
        &["--fail", "refresh=-38:no connection to daemon"],
    );
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("wallet client");
    let e = send::build(
        &ctx,
        &client,
        send::NodeGate::Remote,
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect_err("the gate must refuse");
    assert_eq!(e.exit_code(), 4);
    assert_eq!(e.reason(), Some("unreachable"));
    assert_eq!(
        w.count(&w.wallet_log, "transfer_split"),
        0,
        "nothing was built"
    );
}

/// §2m.3 and §2d: a refresh killed at `timeouts.refresh_sec` is exit **10
/// `wallet_catching_up`** — the same instruction as local mode's "wait, then retry", with no
/// 60 s penalty — and it arms **no breaker**, because `blocks_behind` is unobservable in remote
/// mode and a breaker that fires on the ordinary first sync against a public node is the
/// breaker an agent disables.
#[test]
fn a_refresh_killed_at_its_deadline_is_exit_ten_and_arms_no_breaker() {
    let w = World::new("remote-gate-slow", &[], &["--delay-ms", "3000"]);
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    ctx.timeouts.refresh = Duration::from_secs(1);
    let client = crate::rpc::WalletClient::from_ctx(&ctx).expect("wallet client");
    let e = send::build(
        &ctx,
        &client,
        send::NodeGate::Remote,
        &a_send(DEST),
        &CancelToken::new(),
    )
    .expect_err("the deadline must be enforced");
    assert_eq!(e.exit_code(), 10, "{e}");
    assert_eq!(e.reason(), Some("wallet_catching_up"));
    assert_eq!(
        w.count(&w.wallet_log, "transfer_split"),
        0,
        "nothing was built"
    );
    assert!(
        state::stall_remaining(&ctx, state::Daemon::Wallet)
            .unwrap()
            .is_none(),
        "§2d: no breaker is armed in remote mode"
    );
}

/// §2m.3 and §5: `status` in remote mode constructs no node client, makes no node call, names
/// the address in force, and claims **nothing** about the node — `reachable` is `null`, not
/// `false`, because `false` would be a claim and `true` a guess (§1b.9 item 5).
#[test]
fn status_in_remote_mode_names_the_address_and_claims_nothing_about_the_node() {
    let w = World::new("remote-status", &[], &[]);
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, true);
    let s = crate::status(&ctx).expect("status always answers (§2e)");
    assert_eq!(s.node.layout, crate::units::Layout::Remote);
    assert_eq!(
        s.node.url, "node.example.com:18089",
        "the address IN FORCE (§5)"
    );
    assert!(s.node.trusted, "settings.node.remote_trusted, verbatim");
    assert_eq!(s.node.reachable, None, "null, never false (§5)");
    for field in [
        s.node.height,
        s.node.target_height,
        s.node.tx_pool_size,
        s.node.database_size,
    ] {
        assert!(field.is_none(), "every measured node field is null");
    }
    assert!(s.node.synchronized.is_none() && s.node.restricted.is_none());
    assert!(
        s.node.sync_percent.is_none(),
        "no gauge to draw (§6.4, §7a)"
    );
    // The node mock heard nothing: remote mode makes FEWER calls than local mode, never more.
    assert_eq!(read_log(&w.node_log).len(), 0);
    // The two warnings §5 adds: the mode itself (info) and a trusted non-private host (warn).
    let ids: Vec<&str> = s.warnings.iter().map(|w| w.id.as_str()).collect();
    assert!(ids.contains(&"remote_node"), "{ids:?}");
    assert!(ids.contains(&"remote_node_trusted"), "{ids:?}");
    // And none of the nine node warnings that test a field with no source there.
    for absent in [
        "node_down",
        "node_restricted",
        "node_syncing",
        "node_offline",
        "node_no_peers",
        "node_low_disk",
        "node_update_available",
        "auth_missing",
        "node_stalled",
    ] {
        assert!(!ids.contains(&absent), "{absent} was raised: {ids:?}");
    }
    // `blocks_behind` is null, which is why `last_refresh_at` exists (§2m.3).
    assert!(s.wallet.blocks_behind.is_none());

    // Untrusted — the rule — raises only the info row.
    let mut ctx = w.ctx.clone();
    go_remote(&mut ctx, false);
    let s = crate::status(&ctx).unwrap();
    let ids: Vec<&str> = s.warnings.iter().map(|w| w.id.as_str()).collect();
    assert!(
        ids.contains(&"remote_node") && !ids.contains(&"remote_node_trusted"),
        "{ids:?}"
    );
}

/// §5: `wallet.last_refresh_at` is the last refresh **fmonero** issued, written by the one
/// wrapper all three callers go through — not the daemon's own 20 s auto-refresh, which fmonero
/// cannot observe (§2j).
#[test]
fn the_last_refresh_is_the_one_fmonero_issued() {
    let w = World::new("last-refresh", &[], &[]);
    assert!(
        crate::status(&w.ctx)
            .unwrap()
            .wallet
            .last_refresh_at
            .is_none(),
        "nothing has been issued yet"
    );
    crate::wallet_session::refresh(&w.ctx, &w.wallet(), 0, &CancelToken::new()).unwrap();
    let at = crate::status(&w.ctx)
        .unwrap()
        .wallet
        .last_refresh_at
        .expect("recorded");
    assert!(at.starts_with("20"), "RFC 3339 with a local offset: {at}");
}

// ---- §2i: the mutating-call gate ---------------------------------------------

/// §2i: under a `system_unit` layout the five mutators cannot be obtained at all, and the mock
/// records **zero** requests — the refusal happens before any RPC is issued.
#[test]
fn the_mutators_are_refused_under_a_foreign_layout_with_zero_requests() {
    let w = World::new("gate", &[], &[]);
    let fixture = w.dir.join("systemctl.txt");
    std::fs::write(
        &fixture,
        "fmonero-monerod.service inactive disabled no -\n\
         monerod.service active enabled yes 8421\n",
    )
    .unwrap();
    let mut ctx = w.ctx.clone();
    ctx.systemctl = crate::context::Systemctl::Fixture(fixture.clone());
    assert_eq!(
        crate::units::detect_layout(&ctx).unwrap(),
        crate::units::Layout::SystemUnit
    );
    let n = crate::rpc::NodeClient::from_ctx(&ctx).unwrap();
    let e = match n.mutators(&ctx) {
        Err(e) => e,
        Ok(_) => panic!("§2i: a system_unit layout must not hand out NodeMutators"),
    };
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("foreign_node"));
    assert!(e.hint().contains("sudo systemctl"), "{}", e.hint());
    assert_eq!(read_log(&w.node_log).len(), 0, "§2i: zero requests");
    // Read-only calls still work against any layout.
    assert!(n.get_info().is_ok());
    assert!(n.peers_count().is_ok());
    // `peers_count` is always the READ form (§1b.3).
    for b in w.bodies(&w.node_log, "out_peers") {
        assert_eq!(b["set"], false, "{b}");
    }

    // Under `user_unit` the same mutators are handed out and reach the mock.
    std::fs::write(
        &fixture,
        "fmonero-monerod.service active disabled yes 41233\n\
         monerod.service inactive disabled no -\n",
    )
    .unwrap();
    let m = n.mutators(&ctx).expect("user_unit may mutate");
    m.set_bans(&[crate::rpc::node::BanRequest {
        host: "1.2.3.4".into(),
        ip: 0,
        ban: true,
        seconds: 86_400,
    }])
    .expect("set_bans");
    let body = &w.bodies(&w.node_log, "set_bans")[0];
    assert_eq!(body["bans"][0]["host"], "1.2.3.4");
    assert_eq!(body["bans"][0]["ban"], true);
    // The fixture echoes the node's own answer; 0 means "leave that side alone" (§1b.3).
    let limits = m.set_limit(0, 1024).unwrap();
    assert_eq!((limits.limit_up, limits.limit_down), (1024, 8192));
    let body = &w.bodies(&w.node_log, "set_limit")[0];
    assert_eq!(body["limit_down"], 0);
    assert_eq!(body["limit_up"], 1024);
}

/// §2i: `node up` refuses when **another** monerod holds the data dir, and is idempotent when
/// the holder is our own unit's `MainPID`. The fcntl lock is held by a forked child, because a
/// POSIX record lock is per process.
#[test]
fn node_up_refuses_a_foreign_holder_and_is_idempotent_for_our_own() {
    let w = World::new("node-up", &[], &[]);
    let data_dir = w.dir.join("bitmonero");
    std::fs::create_dir_all(data_dir.join("lmdb")).unwrap();
    let lock = data_dir.join("lmdb/lock.mdb");
    std::fs::write(&lock, b"\0\0\0\0").unwrap();
    let fixture = w.dir.join("systemctl.txt");
    let mut ctx = w.ctx.clone();
    ctx.settings.node.data_dir = data_dir.clone();
    ctx.systemctl = crate::context::Systemctl::Fixture(fixture.clone());

    // A foreign holder: exit 3, naming the pid.
    let child = fork_holding_read_lock(&lock);
    std::fs::write(
        &fixture,
        "fmonero-monerod.service inactive disabled yes 999999\n\
         monerod.service inactive disabled no -\n",
    )
    .unwrap();
    let mut seen = None;
    for _ in 0..200 {
        seen = crate::units::data_dir_in_use(&ctx, &data_dir).unwrap();
        if seen.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(seen, Some(child), "the child's shared lock must be seen");
    let e = crate::node_session::up(&ctx, None, &CancelToken::new()).unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("data_dir_in_use"));
    // X2: the process id moved to `detail`, where `--json` and `-v` keep it; the sentence names
    // the folder, which is the thing a person can act on.
    assert!(
        e.detail().is_some_and(|d| d.contains(&child.to_string())),
        "{e:?}"
    );
    assert!(
        e.to_string().contains("Another program is already using"),
        "{e}"
    );

    // The same holder, now named as our unit's MainPID: exit 0, already running, no second
    // start — which is what §2i's "idempotent" means.
    std::fs::write(
        &fixture,
        format!(
            "fmonero-monerod.service inactive disabled yes {child}\n\
             monerod.service inactive disabled no -\n"
        ),
    )
    .unwrap();
    let started = crate::node_session::up(&ctx, None, &CancelToken::new())
        .expect("our own running node is exit 0");
    assert!(started.already_running);
    assert!(started.reachable);
    // Still inactive in the fixture: no `units::start` was issued.
    assert!(
        !crate::units::state(&ctx, "fmonero-monerod.service")
            .unwrap()
            .active
    );
    reap(child);

    // §2i: `node down` and the mutators require only the LAYOUT, never occupancy — folding
    // the probe into layout detection would refuse about fmonero's own node.
    let child = fork_holding_read_lock(&lock);
    std::thread::sleep(Duration::from_millis(50));
    crate::node_session::down(&ctx, &CancelToken::new()).expect("down needs only the layout");
    assert_eq!(w.count(&w.node_log, "stop_daemon"), 1);
    reap(child);
}

/// §2d: `node up --wait` against a node that never reports `synchronized` is exit **8**, and
/// it is bounded.
#[test]
fn node_up_wait_times_out_with_exit_eight() {
    let w = World::new("node-wait", &["--scenario", "syncing"], &[]);
    let fixture = w.dir.join("systemctl.txt");
    std::fs::write(
        &fixture,
        "fmonero-monerod.service active disabled yes 41233\n\
         monerod.service inactive disabled no -\n",
    )
    .unwrap();
    let mut ctx = w.ctx.clone();
    ctx.systemctl = crate::context::Systemctl::Fixture(fixture);
    ctx.settings.node.data_dir = w.dir.join("never-opened");
    let started = std::time::Instant::now();
    let e = crate::node_session::up(&ctx, Some(Duration::from_secs(2)), &CancelToken::new())
        .unwrap_err();
    assert_eq!(e.exit_code(), 8);
    assert!(e.to_string().contains("still 33% synced"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(6), "bounded (§2d)");
}

/// Fork a child that takes the **shared** `fcntl` lock a live `monerod` holds on byte 0 of
/// `lock.mdb` (`mdb.c:5050`) and then blocks. A POSIX record lock is per **process**, so a lock
/// taken from this process would silently succeed and the test would prove nothing.
///
/// The child closes every inherited descriptor above stderr **before** it opens anything, and
/// uses only `close`/`open`/`fcntl`/`sleep`/`_exit` — all async-signal-safe, which is what a
/// child of a multi-threaded process may call. Closing them is not tidiness: `fork` duplicates
/// the parent's descriptors into the **same** open file descriptions, so a child that kept them
/// would go on holding the `flock` on some other test's `send.lock` for as long as it lived —
/// and that test would then fail with `send_in_progress`, intermittently and somewhere else.
fn fork_holding_read_lock(lock: &std::path::Path) -> i32 {
    use std::os::unix::ffi::OsStrExt;
    let mut c: Vec<u8> = lock.as_os_str().as_bytes().to_vec();
    c.push(0);
    // SAFETY: after the fork the child calls only async-signal-safe functions, over a path
    // buffer that was allocated before it.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        // SAFETY: every call below is async-signal-safe and `c` is NUL-terminated.
        unsafe {
            let mut fd = 3;
            while fd < 1024 {
                libc::close(fd);
                fd += 1;
            }
            let f = libc::open(c.as_ptr() as *const libc::c_char, libc::O_RDWR);
            if f < 0 {
                libc::_exit(8);
            }
            let mut fl = libc::flock {
                l_type: libc::F_RDLCK as libc::c_short,
                l_whence: libc::SEEK_SET as libc::c_short,
                l_start: 0,
                l_len: 1,
                l_pid: 0,
            };
            if libc::fcntl(f, libc::F_SETLK, &mut fl) != 0 {
                libc::_exit(9);
            }
            loop {
                libc::sleep(1);
            }
        }
    }
    pid
}

fn reap(pid: i32) {
    // SAFETY: `pid` is a child of this process.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut st = 0;
        libc::waitpid(pid, &mut st, 0);
    }
}

#[test]
fn selecting_a_node_preserves_the_open_wallet_and_updates_its_live_connection() {
    let mut world = World::new("switch-preserves-wallet", &[], &[]);
    let fixture = world.dir.join("systemctl.txt");
    // spawn_for_test is an in-process RPC thread, not a monero-wallet-rpc
    // systemd process. There is no owned MainPID/global proxy to inherit.
    // Unknown *present* PIDs are covered by the refusal test below.
    let fixture_text = "fmonero-wallet-rpc.service active disabled yes -\n";
    std::fs::write(&fixture, fixture_text).unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture.clone());
    state::set_last_open(&world.ctx, "main", crate::address::NetType::Mainnet).unwrap();
    let changed = crate::node_mode::set_remote(
        &world.ctx,
        "node.example:18081",
        false,
        false,
        &CancelToken::new(),
    )
    .unwrap();
    assert!(!changed.wallet_unit_restarted);
    assert!(changed.wallet_must_reopen.is_none());
    assert_eq!(state::last_open(&world.ctx).unwrap().unwrap().0, "main");
    assert_eq!(std::fs::read_to_string(fixture).unwrap(), fixture_text);
    let calls: Vec<_> = read_log(&world.wallet_log)
        .into_iter()
        .filter(|l| l["method"] == "set_daemon" && l["status"] == 200)
        .map(|l| l["body"]["params"].clone())
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["address"], "node.example:18081");
    assert_eq!(calls[0]["trusted"], false);
    assert_eq!(calls[0]["proxy"], "");
    assert!(calls[0].get("username").is_none());
    assert_eq!(
        world.bodies(&world.wallet_log, "auto_refresh")[0]["enable"],
        true
    );
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
    assert_eq!(world.count(&world.wallet_log, "open_wallet"), 0);
}

#[test]
fn a_synced_local_node_does_not_make_a_behind_wallet_ready_to_send() {
    let world = World::new("send-wallet-behind", &[], &["--scenario", "behind"]);
    let error = send::build(
        &world.ctx,
        &world.wallet(),
        send::NodeGate::Local(&world.node()),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(error.reason(), Some("wallet_catching_up"));
    assert_eq!(world.count(&world.wallet_log, "transfer_split"), 0);
    assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
}

#[test]
fn a_wallet_read_timeout_is_busy_and_does_not_erase_the_open_session() {
    let mut world = World::new("status-wallet-busy", &[], &["--delay-ms", "300"]);
    go_remote(&mut world.ctx, false);
    world.ctx.timeouts.wallet = Duration::from_millis(50);
    state::set_last_open(&world.ctx, "main", crate::address::NetType::Mainnet).unwrap();
    let status = crate::status::status(&world.ctx).unwrap();
    assert!(status.wallet.busy);
    assert!(!status.wallet.reachable);
    assert!(!status
        .warnings
        .iter()
        .any(|w| w.id == "wallet_closed" || w.id == "wallet_idle"));
    assert!(state::last_open(&world.ctx).unwrap().is_some());
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
}

#[test]
fn creating_a_wallet_does_not_reopen_it() {
    let world = World::new(
        "create-once",
        &[],
        &["--fail", "open_wallet=-1:must not reopen"],
    );
    let created = crate::wallet_session::create(
        &world.ctx,
        &world.wallet(),
        "new",
        String::new(),
        "English",
        &|_| {},
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(created.name, "new");
    assert_eq!(world.count(&world.wallet_log, "open_wallet"), 0);
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
    assert_eq!(state::last_open(&world.ctx).unwrap().unwrap().0, "new");
}

#[test]
fn a_rejected_node_change_keeps_the_previous_selection_and_service_settings() {
    let mut world = World::new(
        "switch-rejected",
        &[],
        &["--fail", "set_daemon=-38:rejected"],
    );
    go_remote(&mut world.ctx, false);
    let fixture = world.dir.join("systemctl.txt");
    std::fs::write(
        &fixture,
        // The actual mock is an in-process RPC thread, with no unit MainPID.
        "fmonero-wallet-rpc.service active disabled yes -\n",
    )
    .unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture);
    world.ctx.settings.save(&world.ctx).unwrap();
    crate::units::install(&world.ctx, false).unwrap();
    let config = std::fs::read(world.ctx.paths.config_file()).unwrap();
    let unit_path = world
        .ctx
        .paths
        .systemd_user_dir
        .join(&world.ctx.settings.wallet.unit);
    let unit = std::fs::read(&unit_path).unwrap();
    let error = crate::node_mode::set_remote(
        &world.ctx,
        "rejected.example:18081",
        false,
        false,
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(error.reason(), Some("remote_address_rejected"));
    assert_eq!(
        read_log(&world.wallet_log)
            .iter()
            .filter(|row| row["method"] == "set_daemon" && row["status"] == 200)
            .count(),
        1,
        "the authenticated engine request reaches the injected rejection"
    );
    assert_eq!(world.count(&world.wallet_log, "auto_refresh"), 0);
    assert_eq!(
        std::fs::read(world.ctx.paths.config_file()).unwrap(),
        config
    );
    assert_eq!(std::fs::read(unit_path).unwrap(), unit);
}

#[test]
fn an_unattested_owned_process_refuses_node_change_before_rpc_or_settings_writes() {
    let mut world = World::new("switch-unattested-pid", &[], &[]);
    let fixture = world.dir.join("systemctl.txt");
    // This is the REAL mock host process. Its executable is the test harness,
    // not Monero, so it must not become evidence for an inherited global proxy.
    let fixture_text = format!(
        "fmonero-wallet-rpc.service active disabled yes {}\n",
        std::process::id()
    );
    std::fs::write(&fixture, &fixture_text).unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture.clone());
    world.ctx.settings.node.proxy = Some("127.0.0.1:9050".into());
    world.ctx.settings.save(&world.ctx).unwrap();
    let before = std::fs::read(world.ctx.paths.config_file()).unwrap();
    let error = crate::node_mode::set_remote(
        &world.ctx,
        "node.example:18081",
        false,
        false,
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(error.reason(), Some("transport_policy"));
    assert_eq!(
        std::fs::read(world.ctx.paths.config_file()).unwrap(),
        before
    );
    assert_eq!(std::fs::read_to_string(fixture).unwrap(), fixture_text);
    assert!(
        read_log(&world.wallet_log).is_empty(),
        "no RPC on unknown proxy ownership"
    );
}

#[test]
fn an_unmanaged_mock_receives_the_explicit_proxy_without_claiming_inheritance() {
    let mut world = World::new("switch-explicit-proxy", &[], &[]);
    let fixture = world.dir.join("systemctl.txt");
    std::fs::write(
        &fixture,
        "fmonero-wallet-rpc.service active disabled yes -\n",
    )
    .unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture);
    world.ctx.settings.node.proxy = Some("127.0.0.1:9050".into());
    crate::node_mode::set_remote(
        &world.ctx,
        "node.example:18081",
        false,
        false,
        &CancelToken::new(),
    )
    .unwrap();
    let requests = world.bodies(&world.wallet_log, "set_daemon");
    assert!(!requests.is_empty());
    // Digest may first challenge the client; every wire request carries the
    // same explicit policy, and only one authenticated mutation is attempted.
    for request in &requests {
        assert_eq!(request["proxy"], "127.0.0.1:9050");
        assert_eq!(request["ssl_support"], "enabled");
    }
    assert_eq!(
        read_log(&world.wallet_log)
            .iter()
            .filter(|row| row["method"] == "set_daemon" && row["status"] == 200)
            .count(),
        1,
        "no retry without the requested proxy"
    );
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
    assert_eq!(world.count(&world.wallet_log, "open_wallet"), 0);
}

#[test]
fn a_cancelled_node_change_does_not_save_the_selection() {
    let world = World::new("switch-cancelled", &[], &[]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let error =
        crate::node_mode::set_remote(&world.ctx, "cancelled.example:18081", false, false, &cancel)
            .unwrap_err();
    assert_eq!(error.exit_code(), 130);
    assert!(!world.ctx.paths.config_file().exists());
    assert_eq!(world.count(&world.wallet_log, "set_daemon"), 0);
}

#[test]
fn failed_service_install_does_not_save_a_node_change() {
    let mut world = World::new("switch-install-failed", &[], &[]);
    let fixture = world.dir.join("systemctl.txt");
    std::fs::write(&fixture, "").unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture);
    world.ctx.settings.save(&world.ctx).unwrap();
    let config = std::fs::read(world.ctx.paths.config_file()).unwrap();
    let blocked = world.dir.join("not-a-directory");
    std::fs::write(&blocked, "blocked").unwrap();
    world.ctx.paths.systemd_user_dir = blocked;
    assert!(crate::node_mode::set_remote(
        &world.ctx,
        "unused.example:18081",
        false,
        false,
        &CancelToken::new(),
    )
    .is_err());
    assert_eq!(
        std::fs::read(world.ctx.paths.config_file()).unwrap(),
        config
    );
    assert_eq!(world.count(&world.wallet_log, "set_daemon"), 0);
}

#[test]
fn a_wallet_client_reads_the_login_created_after_the_client() {
    let world = World::new("login-after-client", &[], &[]);
    let path = world
        .ctx
        .paths
        .wallet_login_file(world.ctx.settings.wallet.port);
    std::fs::remove_file(&path).unwrap();
    let client = world.wallet();
    assert_eq!(
        client.get_version().unwrap_err().reason(),
        Some("auth_refused")
    );
    write_login(&path);
    assert!(client.get_version().is_ok());
    assert_eq!(client.authenticated(), Some(true));
}

#[test]
fn a_wallet_client_reloads_a_rotated_login_and_never_falls_back_to_the_old_one() {
    let world = World::new("login-rotation", &[], &[]);
    let path = world
        .ctx
        .paths
        .wallet_login_file(world.ctx.settings.wallet.port);
    std::fs::write(&path, "previous:outdated").unwrap();
    let client = world.wallet();
    assert!(client.get_version().is_err());
    write_login(&path);
    assert!(client.get_version().is_ok());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        client.get_version().unwrap_err().reason(),
        Some("auth_refused")
    );
}

#[test]
fn wallet_controls_refuse_pending_payments_and_stop_preserves_the_node() {
    let mut world = World::new("widget-wallet-controls", &[], &[]);
    let fixture = world.dir.join("systemctl.txt");
    std::fs::write(&fixture, "fmonero-wallet-rpc.service active disabled yes 51002\nfmonero-monerod.service active disabled yes 51003\n").unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture);
    state::set_last_open(&world.ctx, "main", crate::address::NetType::Mainnet).unwrap();
    state::set_pending_send(
        &world.ctx,
        &PendingSend {
            kind: PendingKind::Send,
            account_index: 0,
            dest: DEST.into(),
            built_at: crate::now_unix(),
            txs: vec![PendingTx {
                txid: BUILT_TXID.into(),
                amount: Piconero(1),
                fee: Piconero(2),
                relayed: false,
                relayed_at: None,
            }],
        },
    )
    .unwrap();
    let cancel = CancelToken::new();
    let session = crate::app::Session::enter(&world.ctx).unwrap();
    assert_eq!(
        crate::app::stop(&world.ctx, &cancel).unwrap_err().reason(),
        Some("pending_send")
    );
    assert!(!session.stop_requested(&world.ctx).unwrap());
    {
        let lock = crate::send::take_send_lock(&world.ctx).unwrap();
        assert_eq!(
            crate::app::stop(&world.ctx, &cancel).unwrap_err().reason(),
            Some("send_in_progress")
        );
        drop(lock);
    }
    assert_eq!(
        crate::wallet_session::stop(&world.ctx, &cancel)
            .unwrap_err()
            .reason(),
        Some("pending_send")
    );
    assert_eq!(
        crate::wallet_session::close(&world.ctx, &world.wallet())
            .unwrap_err()
            .reason(),
        Some("pending_send")
    );
    assert_eq!(
        crate::wallet_session::open(&world.ctx, &world.wallet(), "other", String::new(), &cancel)
            .unwrap_err()
            .reason(),
        Some("pending_send")
    );
    assert!(
        crate::units::state(&world.ctx, &world.ctx.settings.wallet.unit)
            .unwrap()
            .active
    );
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
    assert_eq!(world.count(&world.wallet_log, "open_wallet"), 0);
    assert!(state::last_open(&world.ctx).unwrap().is_some());
    state::clear_pending_send(&world.ctx).unwrap();
    crate::wallet_session::stop(&world.ctx, &cancel).unwrap();
    assert!(
        !crate::units::state(&world.ctx, &world.ctx.settings.wallet.unit)
            .unwrap()
            .active
    );
    assert!(
        crate::units::state(&world.ctx, &world.ctx.settings.node.unit)
            .unwrap()
            .active
    );
    assert!(state::last_open(&world.ctx).unwrap().is_none());
}

#[test]
fn successful_open_validates_account_selection_and_preserves_other_settings() {
    let world = World::new("open-account-reset", &[], &[]);
    let mut settings = world.ctx.settings.clone();
    settings.wallet.account = 1;
    settings.tui.privacy = true;
    settings.tui.notifications = true;
    settings.save(&world.ctx).unwrap();
    crate::wallet_session::open_secret(
        &world.ctx,
        &world.wallet(),
        "new-scratch",
        crate::SecretString::new("test-only".into()).unwrap(),
        &CancelToken::new(),
    )
    .unwrap();
    let saved = crate::Settings::load(&world.ctx.paths).unwrap();
    assert_eq!(saved.wallet.account, 0);
    assert!(saved.tui.privacy);
    assert!(saved.tui.notifications);
    assert_eq!(saved.wallet.url, settings.wallet.url);
    assert_eq!(world.count(&world.wallet_log, "get_accounts"), 1);
    assert!(world
        .bodies(&world.wallet_log, "get_address")
        .iter()
        .all(|p| p["account_index"] == 0));
}

#[test]
fn failed_account_validation_never_claims_a_ready_wallet_or_rewrites_selection() {
    let world = World::new(
        "open-account-fail",
        &[],
        &["--fail", "get_accounts=-1:unavailable"],
    );
    let mut settings = world.ctx.settings.clone();
    settings.wallet.account = 7;
    settings.save(&world.ctx).unwrap();
    assert!(crate::wallet_session::open_secret(
        &world.ctx,
        &world.wallet(),
        "new-scratch",
        crate::SecretString::new("test-only".into()).unwrap(),
        &CancelToken::new()
    )
    .is_err());
    assert_eq!(
        crate::Settings::load(&world.ctx.paths)
            .unwrap()
            .wallet
            .account,
        7
    );
    assert!(state::last_open(&world.ctx).unwrap().is_none());
}

#[test]
fn unconfirmed_restore_clears_previous_identity_and_never_retries_or_configures() {
    let world = World::new(
        "restore-unknown",
        &[],
        &["--fail", "restore_deterministic_wallet=-1:refused"],
    );
    state::set_last_open(&world.ctx, "previous", crate::NetType::Mainnet).unwrap();
    let request = crate::restore::RestoreRequest {
        name: "new-scratch".into(),
        seed: crate::SecretString::new(crate::rpc::random_hex(16).unwrap()).unwrap(),
        password: crate::SecretString::new(crate::rpc::random_hex(16).unwrap()).unwrap(),
        seed_offset: None,
        restore_height: 456,
        language: Some("English".into()),
    };
    let result =
        crate::restore::restore(&world.ctx, &world.wallet(), request, &CancelToken::new()).unwrap();
    assert_eq!(result.state, crate::restore::RestoreState::Unknown);
    assert!(state::last_open(&world.ctx).unwrap().is_none());
    assert_eq!(
        world.count(&world.wallet_log, "restore_deterministic_wallet"),
        1
    );
    assert_eq!(world.count(&world.wallet_log, "set_daemon"), 0);
    assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
    let bodies = world.bodies(&world.wallet_log, "restore_deterministic_wallet");
    assert_eq!(bodies[0]["restore_height"], 456);
    assert_eq!(bodies[0]["autosave_current"], true);
    assert_eq!(bodies[0]["enable_multisig_experimental"], false);
}

#[test]
fn restore_local_preconditions_refuse_before_any_rpc() {
    let world = World::new("restore-preconditions", &[], &[]);
    std::fs::create_dir_all(&world.ctx.settings.wallet.dir).unwrap();
    std::fs::write(
        world.ctx.settings.wallet.dir.join("existing.keys"),
        b"test metadata only",
    )
    .unwrap();
    let request = crate::restore::RestoreRequest {
        name: "existing".into(),
        seed: crate::SecretString::empty(),
        password: crate::SecretString::empty(),
        seed_offset: None,
        restore_height: 0,
        language: None,
    };
    assert!(
        crate::restore::restore(&world.ctx, &world.wallet(), request, &CancelToken::new()).is_err()
    );
    assert!(read_log(&world.wallet_log).is_empty());
}

#[test]
fn split_payment_preserves_every_metadata_item_and_rejects_inconsistent_arrays() {
    // Isolated wire-format fault cases derived from the checked-in Monero response
    // schema; these are not evidence of transaction creation by a real backend.
    let (scratch, split) = split_send_fixtures();
    let file = scratch.join("wallet/transfer_split-splitcase.json");
    let scratch_text = scratch.to_string_lossy().into_owned();
    let world = World::new(
        "split-two",
        &[],
        &["--fixtures", &scratch_text, "--scenario", "splitcase"],
    );
    let client = world.wallet();
    let node = world.node();
    let cancel = CancelToken::new();
    let plan = send::build(
        &world.ctx,
        &client,
        send::NodeGate::Local(&node),
        &a_send(DEST),
        &cancel,
    )
    .unwrap();
    assert_eq!(plan.txs.len(), 2);
    assert_eq!(plan.total_amount, Piconero(100_000_000_000));
    let expected: Vec<_> = plan
        .txs
        .iter()
        .map(|tx| (tx.txid.clone(), tx.metadata.clone()))
        .collect();
    let txids = send::relay(&world.ctx, &client, plan, &|_| {}, &cancel).unwrap();
    assert_eq!(
        txids,
        expected.iter().map(|p| p.0.clone()).collect::<Vec<_>>()
    );
    let relayed = world.bodies(&world.wallet_log, "relay_tx");
    assert_eq!(relayed.len(), 2);
    for (body, (_, metadata)) in relayed.iter().zip(&expected) {
        assert_eq!(body["hex"].as_str(), Some(metadata.as_str()));
    }
    assert_eq!(world.count(&world.wallet_log, "transfer_split"), 1);
    assert_eq!(world.count(&world.wallet_log, "transfer"), 0);
    drop(world);
    for case in [
        "short-metadata",
        "duplicate-id",
        "fee-overflow",
        "weight-overflow",
    ] {
        let mut bad = split.clone();
        match case {
            "short-metadata" => {
                bad["tx_metadata_list"].as_array_mut().unwrap().pop();
            }
            "duplicate-id" => bad["tx_hash_list"][1] = bad["tx_hash_list"][0].clone(),
            "fee-overflow" => bad["fee_list"] = serde_json::json!([u64::MAX, 1]),
            "weight-overflow" => bad["weight_list"] = serde_json::json!([u64::MAX, 1]),
            _ => unreachable!(),
        }
        std::fs::write(&file, serde_json::to_vec(&bad).unwrap()).unwrap();
        let world = World::new(
            case,
            &[],
            &["--fixtures", &scratch_text, "--scenario", "splitcase"],
        );
        assert!(
            send::build(
                &world.ctx,
                &world.wallet(),
                send::NodeGate::Local(&world.node()),
                &a_send(DEST),
                &CancelToken::new()
            )
            .is_err(),
            "{case}"
        );
        assert_eq!(world.count(&world.wallet_log, "transfer_split"), 1);
        assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

fn split_send_fixtures() -> (PathBuf, serde_json::Value) {
    let scratch = std::env::temp_dir().join(format!(
        "fmonero-split-fixtures-{}-{}",
        std::process::id(),
        crate::rpc::random_hex(8).unwrap()
    ));
    let wallet_dir = scratch.join("wallet");
    std::fs::create_dir_all(&wallet_dir).unwrap();
    for entry in std::fs::read_dir(fixtures().join("wallet")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), wallet_dir.join(entry.file_name())).unwrap();
        }
    }
    let mut split: serde_json::Value =
        serde_json::from_slice(&std::fs::read(wallet_dir.join("sweep_all.json")).unwrap()).unwrap();
    split["amount_list"] = serde_json::json!([60_000_000_000u64, 40_000_000_000u64]);
    let file = wallet_dir.join("transfer_split-splitcase.json");
    std::fs::write(&file, serde_json::to_vec(&split).unwrap()).unwrap();
    (scratch, split)
}

#[test]
fn label_and_note_mutations_refuse_changed_review_state() {
    let world = World::new("stale-reviewed-edits", &[], &[]);
    state::set_last_open(&world.ctx, "scratch", crate::NetType::Mainnet).unwrap();
    let client = world.wallet();
    let identity = crate::wallet_ops::identity(&world.ctx, &client).unwrap();
    let mut receiving = crate::receive::select(&world.ctx, &client, &identity, 0, 0).unwrap();
    receiving.label.push_str(" changed since review");
    assert!(crate::receive::rename(&world.ctx, &client, &receiving, "new label").is_err());
    assert_eq!(world.count(&world.wallet_log, "label_address"), 0);
    assert!(crate::history::set_note(
        &world.ctx,
        &client,
        &identity,
        BUILT_TXID,
        0,
        "a prior note that the fixture does not have",
        "replacement"
    )
    .is_err());
    assert_eq!(world.count(&world.wallet_log, "set_tx_notes"), 0);
}

fn one_off_world(name: &str) -> World {
    let mut world = World::new(name, &[], &[]);
    let fixture = world.dir.join("systemctl.txt");
    // The mock is an in-process server, not an owned Monero unit/global proxy.
    std::fs::write(
        &fixture,
        "fmonero-wallet-rpc.service active disabled yes -\n",
    )
    .unwrap();
    world.ctx.systemctl = crate::context::Systemctl::Fixture(fixture);
    world.wallet().open_wallet("scratch", "").unwrap();
    state::set_last_open(&world.ctx, "scratch", crate::NetType::Mainnet).unwrap();
    world.ctx.settings.save(&world.ctx).unwrap();
    world
}

#[test]
fn one_off_node_change_normalizes_local_auth_and_never_persists_settings() {
    let mut world = one_off_world("one-off-auth");
    crate::write_private(
        &world.ctx.paths.monerod_conf(),
        b"rpc-login=scratch-node:fixture-only\n",
    )
    .unwrap();
    let client = world.wallet();
    let identity = crate::wallet_ops::identity(&world.ctx, &client).unwrap();
    let config = std::fs::read(world.ctx.paths.config_file()).unwrap();
    for address in [
        "127.0.0.1:18081",
        "http://127.0.0.1:18081",
        "https://127.0.0.1:18081",
        "http://[::1]:18081",
    ] {
        world.ctx.settings.node.fingerprint =
            address.starts_with("https://").then(|| "ab".repeat(32));
        let connection =
            crate::transport::configure_once(&world.ctx, &client, &identity, address, true)
                .unwrap();
        assert_eq!(connection.address(), address);
        let rows = read_log(&world.wallet_log);
        let call = rows
            .iter()
            .rev()
            .find(|r| r["method"] == "set_daemon" && r["status"] == 200)
            .unwrap();
        assert_eq!(call["body"]["params"]["username"], "scratch-node");
        assert_eq!(call["body"]["params"]["password"], "fixture-only");
        assert_eq!(
            call["body"]["params"]["ssl_support"],
            if address.starts_with("https://") {
                "enabled"
            } else {
                "disabled"
            }
        );
        assert_eq!(
            std::fs::read(world.ctx.paths.config_file()).unwrap(),
            config
        );
    }
    world.ctx.settings.node.fingerprint = None;
    crate::transport::configure_once(&world.ctx, &client, &identity, "node.example:18081", false)
        .unwrap();
    let rows = read_log(&world.wallet_log);
    let call = rows
        .iter()
        .rev()
        .find(|r| r["method"] == "set_daemon" && r["status"] == 200)
        .unwrap();
    assert!(call["body"]["params"].get("username").is_none());
    assert!(call["body"]["params"].get("password").is_none());
    assert_eq!(
        std::fs::read(world.ctx.paths.config_file()).unwrap(),
        config
    );
    assert_eq!(state::last_open(&world.ctx).unwrap().unwrap().0, "scratch");
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
}

#[test]
fn one_off_node_and_metadata_mutations_refuse_locks_pending_and_stale_identity() {
    let world = one_off_world("one-off-guards");
    let client = world.wallet();
    let identity = crate::wallet_ops::identity(&world.ctx, &client).unwrap();
    let refuse_all = || {
        assert!(crate::transport::configure_once(
            &world.ctx,
            &client,
            &identity,
            "node.example:18081",
            false
        )
        .is_err());
        assert!(crate::receive::new_address(&world.ctx, &client, &identity, 0, "invoice").is_err());
        assert!(crate::history::set_note(
            &world.ctx, &client, &identity, BUILT_TXID, 0, "", "reviewed"
        )
        .is_err());
        for method in ["set_daemon", "create_address", "set_tx_notes"] {
            assert_eq!(
                world.count(&world.wallet_log, method),
                0,
                "{method} must not mutate"
            );
        }
    };
    let held = send::take_send_lock(&world.ctx).unwrap();
    refuse_all();
    drop(held);
    for name in ["pending.json", "native-pending.json"] {
        let path = world.ctx.paths.state_dir.join(name);
        std::fs::write(&path, b"unreadable transaction state").unwrap();
        refuse_all();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"unreadable transaction state"
        );
        std::fs::remove_file(path).unwrap();
    }
    state::set_last_open(&world.ctx, "replacement", crate::NetType::Mainnet).unwrap();
    refuse_all();
    assert_eq!(
        crate::receive::addresses(&world.ctx, &client, &identity, 0)
            .unwrap_err()
            .reason(),
        Some("wallet_changed")
    );
}

#[test]
fn empty_or_whitespace_payment_ledger_blocks_metadata_mutations() {
    let world = one_off_world("empty-ledger-metadata");
    let client = world.wallet();
    let identity = crate::wallet_ops::identity(&world.ctx, &client).unwrap();
    let receiving = crate::receive::select(&world.ctx, &client, &identity, 0, 0).unwrap();
    let path = world.ctx.paths.pending_file();

    for bytes in [b"".as_slice(), b" \t\r\n".as_slice()] {
        crate::write_private(&path, bytes).unwrap();
        let before = read_log(&world.wallet_log).len();
        let errors = [
            crate::receive::new_address(&world.ctx, &client, &identity, 0, "invoice").unwrap_err(),
            crate::receive::rename(&world.ctx, &client, &receiving, "reviewed").unwrap_err(),
            crate::history::set_note(
                &world.ctx, &client, &identity, BUILT_TXID, 0, "", "reviewed",
            )
            .unwrap_err(),
        ];
        for error in errors {
            assert_eq!(error.exit_code(), 3);
            assert_eq!(error.reason(), Some("pending_send"));
        }
        assert_eq!(read_log(&world.wallet_log).len(), before);
        for method in ["create_address", "label_address", "set_tx_notes"] {
            assert_eq!(world.count(&world.wallet_log, method), 0, "{method}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        state::clear_pending_send(&world.ctx).unwrap();
        assert!(!path.exists());
        drop(crate::wallet_ops::guard(&world.ctx, &client, &identity).unwrap());
    }
}

#[test]
fn one_off_node_change_enforces_real_process_proxy_attestation() {
    let mut world = one_off_world("one-off-proxy");
    let fixture = world.dir.join("systemctl.txt");
    // Actual mock-host PID, deliberately not a monero-wallet-rpc executable.
    std::fs::write(
        &fixture,
        format!(
            "fmonero-wallet-rpc.service active disabled yes {}\n",
            std::process::id()
        ),
    )
    .unwrap();
    world.ctx.settings.node.proxy = Some("127.0.0.1:9050".into());
    world.ctx.settings.node.fingerprint = Some("ab".repeat(32));
    let client = world.wallet();
    let identity = crate::wallet_ops::identity(&world.ctx, &client).unwrap();
    let config = std::fs::read(world.ctx.paths.config_file()).unwrap();
    let error = crate::transport::configure_once(
        &world.ctx,
        &client,
        &identity,
        "https://node.example:18081",
        false,
    )
    .unwrap_err();
    assert_eq!(error.reason(), Some("transport_policy"));
    assert_eq!(world.count(&world.wallet_log, "set_daemon"), 0);
    assert_eq!(
        std::fs::read(world.ctx.paths.config_file()).unwrap(),
        config
    );
    // Without an owned MainPID this actual in-process mock gets explicit proxy
    // parameters, never fabricated evidence for an inherited Monero global proxy.
    std::fs::write(
        &fixture,
        "fmonero-wallet-rpc.service active disabled yes -\n",
    )
    .unwrap();
    crate::transport::configure_once(
        &world.ctx,
        &client,
        &identity,
        "https://node.example:18081",
        false,
    )
    .unwrap();
    let calls: Vec<_> = read_log(&world.wallet_log)
        .into_iter()
        .filter(|r| r["method"] == "set_daemon" && r["status"] == 200)
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["body"]["params"]["proxy"], "127.0.0.1:9050");
    assert_eq!(calls[0]["body"]["params"]["ssl_support"], "enabled");
    assert_eq!(
        std::fs::read(world.ctx.paths.config_file()).unwrap(),
        config
    );
}

#[test]
fn payment_ledger_directory_failure_prevents_first_relay() {
    use state::payment_io_fault::{self, Stage};
    for stage in [Stage::Open, Stage::Sync] {
        let world = World::new(&format!("ledger-first-{stage:?}"), &[], &[]);
        let client = world.wallet();
        let node = world.node();
        let cancel = CancelToken::new();
        let plan = send::build(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &a_send(DEST),
            &cancel,
        )
        .unwrap();
        let _fault = payment_io_fault::inject(&world.ctx.paths.state_dir, stage, 0);
        // Cache persistence retains its own behavior and cannot consume this gate.
        state::note_refresh(&world.ctx).unwrap();
        let error = send::relay(&world.ctx, &client, plan, &|_| {}, &cancel).unwrap_err();
        assert!(
            error.to_string().contains("payment record directory"),
            "{error}"
        );
        assert!(error.detail().unwrap().contains("os error 5"));
        assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
        assert_eq!(world.count(&world.wallet_log, "store"), 0);
        // Rename happened, but its durability is unconfirmed: never erase the guard.
        let record = state::pending_send(&world.ctx).unwrap().unwrap();
        assert_eq!(record.txs.len(), 1);
        assert_eq!(record.txs[0].txid, BUILT_TXID);
        assert!(!record.txs[0].relayed);
        assert!(record.txs[0].relayed_at.is_none());
        assert_eq!(
            send::refuse_if_send_in_flight(&world.ctx)
                .unwrap_err()
                .reason(),
            Some("pending_send")
        );
        let before = world.count(&world.wallet_log, "transfer_split");
        assert!(send::build(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &a_send(DEST),
            &cancel
        )
        .is_err());
        assert_eq!(world.count(&world.wallet_log, "transfer_split"), before);
        assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
    }
}

#[test]
fn payment_ledger_directory_failure_after_ack_keeps_partial_state_and_stops_relay() {
    use state::payment_io_fault::{self, Stage};
    let (scratch, _) = split_send_fixtures();
    let fixture_path = scratch.to_string_lossy().into_owned();
    for stage in [Stage::Open, Stage::Sync] {
        let world = World::new(
            &format!("ledger-after-ack-{stage:?}"),
            &[],
            &["--fixtures", &fixture_path, "--scenario", "splitcase"],
        );
        let client = world.wallet();
        let node = world.node();
        let cancel = CancelToken::new();
        let plan = send::build(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &a_send(DEST),
            &cancel,
        )
        .unwrap();
        assert_eq!(plan.txs.len(), 2);
        assert!(
            !plan.is_sweep,
            "ordinary split payment uses the same durable gate"
        );
        let ids: Vec<_> = plan.txs.iter().map(|t| t.txid.clone()).collect();
        let metadata = plan.txs[0].metadata.clone();
        // Let the entire initial record become durable. Fail the directory I/O
        // only after Monero's first successful acknowledgement and the next rename.
        let _fault = payment_io_fault::inject(&world.ctx.paths.state_dir, stage, 1);
        let error = send::relay(&world.ctx, &client, plan, &|_| {}, &cancel).unwrap_err();
        assert!(
            error.to_string().contains("accepted 1 of 2 payments"),
            "{error}"
        );
        assert!(error.to_string().contains("Do not send it again"));
        assert_eq!(error.reason(), Some("pending_send"));
        assert!(error.detail().unwrap().contains("os error 5"));
        assert!(!error.hint().contains("clear-pending"));
        assert!(error.action().unwrap().contains("Inspect wallet history"));
        let relayed = world.bodies(&world.wallet_log, "relay_tx");
        assert_eq!(relayed.len(), 1);
        assert_eq!(relayed[0]["hex"].as_str(), Some(metadata.as_str()));
        assert_eq!(world.count(&world.wallet_log, "store"), 0);
        let bytes = std::fs::read(world.ctx.paths.pending_file()).unwrap();
        let record = state::pending_send(&world.ctx).unwrap().unwrap();
        assert_eq!(
            record
                .txs
                .iter()
                .map(|t| t.txid.clone())
                .collect::<Vec<_>>(),
            ids
        );
        assert!(record.txs[0].relayed);
        assert!(record.txs[0].relayed_at.is_some());
        assert!(!record.txs[1].relayed);
        assert!(record.txs[1].relayed_at.is_none());
        assert_eq!(
            send::refuse_if_send_in_flight(&world.ctx)
                .unwrap_err()
                .reason(),
            Some("pending_send")
        );
        assert!(send::build(
            &world.ctx,
            &client,
            send::NodeGate::Local(&node),
            &a_send(DEST),
            &cancel
        )
        .is_err());
        assert_eq!(world.count(&world.wallet_log, "transfer_split"), 1);
        assert_eq!(world.count(&world.wallet_log, "relay_tx"), 1);
        assert_eq!(
            std::fs::read(world.ctx.paths.pending_file()).unwrap(),
            bytes
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn payment_ledger_directory_failure_during_clear_is_reported() {
    use state::payment_io_fault::{self, Stage};
    for stage in [Stage::Open, Stage::Sync] {
        let world = World::new(&format!("ledger-clear-{stage:?}"), &[], &[]);
        state::set_pending_send(
            &world.ctx,
            &PendingSend {
                kind: PendingKind::Send,
                account_index: 0,
                dest: DEST.into(),
                built_at: crate::now_unix(),
                txs: vec![PendingTx {
                    txid: BUILT_TXID.into(),
                    amount: Piconero(100_000_000_000),
                    fee: Piconero(30_720_000),
                    relayed: true,
                    relayed_at: Some(crate::now_unix()),
                }],
            },
        )
        .unwrap();
        let _fault = payment_io_fault::inject(&world.ctx.paths.state_dir, stage, 0);
        let error = state::clear_pending_send(&world.ctx).unwrap_err();
        assert!(error.to_string().contains("payment record directory"));
        assert!(error.detail().unwrap().contains("os error 5"));
        // Unlink is visible, but durable removal is not reported as successful.
        assert!(!world.ctx.paths.pending_file().exists());
        assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
    }
}

#[test]
fn native_empty_key_image_response_exports_an_explicit_empty_artifact() {
    use std::os::unix::fs::PermissionsExt;
    // Replay the offset-only schema observed in the isolated native 0.18.5.3
    // harness through protected RPC and the actual guarded artifact exporter.
    let scratch = std::env::temp_dir().join(format!(
        "fmonero-empty-image-fixtures-{}-{}",
        std::process::id(),
        crate::rpc::random_hex(8).unwrap()
    ));
    let wallet_dir = scratch.join("wallet");
    std::fs::create_dir_all(&wallet_dir).unwrap();
    for entry in std::fs::read_dir(fixtures().join("wallet")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), wallet_dir.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(
        wallet_dir.join("export_key_images.json"),
        br#"{"offset":0}"#,
    )
    .unwrap();
    let fixture_path = scratch.to_string_lossy().into_owned();
    let world = World::new("empty-key-images", &[], &["--fixtures", &fixture_path]);
    state::set_last_open(&world.ctx, "scratch", crate::NetType::Mainnet).unwrap();
    let client = world.wallet();
    let identity = crate::wallet_ops::identity(&world.ctx, &client).unwrap();
    let output = world.dir.join("empty-images.json");
    crate::watch::export_key_images(&world.ctx, &client, &identity, &output).unwrap();
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(artifact["format"], "fmonero.key-images/1");
    assert_eq!(artifact["address"], identity.primary_address);
    assert_eq!(
        artifact["images"],
        serde_json::json!({"offset":0,"signed_key_images":[]})
    );
    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        world.bodies(&world.wallet_log, "export_key_images"),
        vec![serde_json::json!({"all":true})]
    );
    assert_eq!(world.count(&world.wallet_log, "import_key_images"), 0);
    assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn wallet_unknown_error_refresh_preserves_upstream_cause_and_session() {
    let world = World::new(
        "refresh-unknown-error",
        &[],
        &[
            "--fail",
            "refresh=-1:Unexpected hard fork version v16 at height 1",
        ],
    );
    state::set_last_open(&world.ctx, "scratch", crate::NetType::Mainnet).unwrap();
    let error = world.wallet().refresh(0, &CancelToken::new()).unwrap_err();
    assert_eq!(error.exit_code(), 5);
    assert_eq!(error.reason(), Some("rpc_error"));
    assert_eq!(
        error.to_string(),
        "Unexpected hard fork version v16 at height 1"
    );
    assert_eq!(state::last_open(&world.ctx).unwrap().unwrap().0, "scratch");
    assert_eq!(
        read_log(&world.wallet_log)
            .iter()
            .filter(|row| row["method"] == "refresh" && row["status"] == 200)
            .count(),
        1
    );
    assert_eq!(world.count(&world.wallet_log, "close_wallet"), 0);
    assert_eq!(world.count(&world.wallet_log, "relay_tx"), 0);
}

#[test]
fn wallet_unknown_error_protected_open_keeps_password_refusal_and_redaction() {
    let secret = crate::SecretString::new(crate::rpc::random_hex(24).unwrap()).unwrap();
    let failure = format!(
        "open_wallet=-1:Failed to open wallet : {}",
        secret.expose_secret()
    );
    let world = World::new("open-unknown-error", &[], &["--fail", &failure]);
    let error = world
        .wallet()
        .open_wallet("scratch", secret.expose_secret())
        .unwrap_err();
    assert_eq!(error.exit_code(), 9);
    assert_eq!(error.reason(), Some("bad_password"));
    assert!(!format!("{error:?}").contains(secret.expose_secret()));
    assert!(!error.to_string().contains(secret.expose_secret()));
    assert!(!error.hint().contains(secret.expose_secret()));
    assert_eq!(
        read_log(&world.wallet_log)
            .iter()
            .filter(|row| row["method"] == "open_wallet" && row["status"] == 200)
            .count(),
        1
    );
    assert_eq!(world.count(&world.wallet_log, "set_daemon"), 0);
    assert_eq!(world.count(&world.wallet_log, "auto_refresh"), 0);
}

#[test]
fn wallet_unknown_error_protected_creation_is_not_a_password_failure() {
    for (code, exit, reason) in [(-1, 5, "rpc_error"), (-22, 9, "bad_password")] {
        let failure = format!("create_wallet={code}:private peer detail must not escape");
        let world = World::new(&format!("create-error-{code}"), &[], &["--fail", &failure]);
        let error = world
            .wallet()
            .create_wallet("scratch", "", "English")
            .unwrap_err();
        assert_eq!(error.exit_code(), exit);
        assert_eq!(error.reason(), Some(reason));
        assert!(!format!("{error:?}").contains("private peer detail must not escape"));
        assert_eq!(
            read_log(&world.wallet_log)
                .iter()
                .filter(|row| row["method"] == "create_wallet" && row["status"] == 200)
                .count(),
            1
        );
        assert_eq!(world.count(&world.wallet_log, "open_wallet"), 0);
    }
}
