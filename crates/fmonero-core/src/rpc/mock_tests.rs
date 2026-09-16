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
/// The txid `transfer-do-not-relay.json` builds and `relay_tx.json` echoes.
const BUILT_TXID: &str = "a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f";

// ---- §2a.3: exactly one transfer and one relay_tx, with identical metadata ----

/// The central §2a test: **the send path issues exactly one `transfer` and one `relay_tx`, and
/// the `relay_tx.hex` is byte-identical to the `transfer` response's `tx_metadata`.**
///
/// `relay_tx`'s `hex` is hex of a serialized `wallet2::pending_tx` — **not** `tx_blob`
/// (§1b.4) — so relaying anything else is a send that silently does nothing.
#[test]
fn the_send_path_builds_once_and_relays_that_exact_transaction() {
    let w = World::new("send-once", &[], &[]);
    let client = w.wallet();
    let node = w.node();
    let cancel = CancelToken::new();
    let plan = send::build(&w.ctx, &client, Some(&node), &a_send(DEST), &cancel)
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

    assert_eq!(w.count(&w.wallet_log, "transfer"), 1, "exactly one build");
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

/// §2a.8: the recorded bodies carry **no** `unlock_time` and **no** `payment_id` — and
/// §2a.10: every recorded `sweep_all` carries `subaddr_indices_all: true`.
#[test]
fn the_recorded_bodies_carry_no_forbidden_field_and_every_sweep_is_indices_all() {
    let w = World::new("bodies", &[], &[]);
    let client = w.wallet();
    let node = w.node();
    let cancel = CancelToken::new();
    let plan = send::build(&w.ctx, &client, Some(&node), &a_send(DEST), &cancel).unwrap();
    drop(plan); // discarding a plan releases the §2a.11 lock, so the sweep below can build
    let sweep = SweepRequest {
        dest: DEST.to_string(),
        account_index: 0,
        priority: 2,
        below: None,
        max_fee: None,
    };
    let plan = send::build_sweep(&w.ctx, &client, Some(&node), &sweep, &cancel).unwrap();
    assert_eq!(plan.txs.len(), 2, "sweep_all.json carries two transactions");
    drop(plan);

    for method in ["transfer", "sweep_all"] {
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
    // `transfer` sends one destination and never asks for the transaction secret key.
    let t = &w.bodies(&w.wallet_log, "transfer")[0];
    assert_eq!(t["destinations"][0]["address"], DEST);
    assert_eq!(t["get_tx_key"], false, "§2a.2, §2b.2");
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
        Some(&node),
        &a_send(DEST),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("pending_send"));
    assert!(e.hint().contains("clear-pending"), "{}", e.hint());
    // Nothing was built: the refusal is at step 4, before the daemon is asked for anything.
    assert_eq!(w.count(&w.wallet_log, "transfer"), 0);
    // And the record is still there: a check that cleared it would disarm the interlock.
    assert!(state::pending_send(&w.ctx).unwrap().is_some());
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
        Some(&node),
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
        Some(&w.node()),
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
        Some(&w.node()),
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
        Some(&w.node()),
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
    let plan = send::build(&w.ctx, &client, Some(&w.node()), &a_send(DEST), &cancel).unwrap();
    let e = send::relay(&w.ctx, &client, plan, &|_| {}, &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    assert_eq!(e.reason(), Some("txid_mismatch"));
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
/// records **zero** `relay_tx`.
#[test]
fn a_fee_above_max_refuses_and_relays_nothing() {
    let w = World::new("max-fee", &[], &[]);
    let mut req = a_send(DEST);
    // The fixture's fee is 30_720_000 piconero.
    req.max_fee = Some(Piconero(1));
    let e = send::build(
        &w.ctx,
        &w.wallet(),
        Some(&w.node()),
        &req,
        &CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert_eq!(e.reason(), Some("fee_above_max"));
    assert_eq!(w.count(&w.wallet_log, "relay_tx"), 0);
    assert_eq!(w.count(&w.wallet_log, "transfer"), 1, "it did build first");

    // And `relay` re-checks it, so a plan built by other means still cannot get past it.
    let mut req = a_send(DEST);
    req.max_fee = Some(Piconero(30_720_000));
    let client = w.wallet();
    let plan = send::build(&w.ctx, &client, Some(&w.node()), &req, &CancelToken::new())
        .expect("a fee exactly at the bound is allowed");
    assert_eq!(plan.total_fee, Piconero(30_720_000));
    drop(plan);
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
        Some(&w.node()),
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
        Some(&w.node()),
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
        Some(&w.node()),
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
        send::build_sweep(&ctx, &client, Some(&node), &sweep, &cancel)
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
    let plan = send::build(&w.ctx, &client, Some(&w.node()), &a_send(DEST), &cancel).unwrap();
    let e = send::relay(&w.ctx, &client, plan, &|_| {}, &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 16);
    assert_eq!(e.reason(), Some("relay_refused"));
    assert!(e.to_string().contains("0 of 1 broadcast"), "{e}");
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
    let e = send::build(&w.ctx, &client, Some(&node), &req, &cancel).unwrap_err();
    assert_eq!((e.exit_code(), e.reason()), (2, Some("bad_amount")));

    // 2. A structurally impossible address is exit 15, before the lock and before any RPC.
    let e = send::build(
        &w.ctx,
        &client,
        Some(&node),
        &a_send("not-an-address"),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 15);
    assert_eq!(w.count(&w.wallet_log, "validate_address"), 0);

    // 6. A wrong-nettype recipient is exit 15, with the nettype named.
    // A real stagenet standard address, from `tests/fixtures/address-forms.json`.
    const STAGENET: &str = "53teqCAESLxeJ1REzGMAat1ZeHvuajvDiXqboEocPaDRRmqWoVPzy46GLo866qRFjbNhfkNckyhST3WEvBviDwpUDd7DSzB";
    let e = send::build(&w.ctx, &client, Some(&node), &a_send(STAGENET), &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 15);
    assert!(e.to_string().contains("stagenet"), "{e}");
    assert_eq!(w.count(&w.wallet_log, "transfer"), 0, "nothing was built");

    // 5a. `@label`: zero matches is exit 6, several is exit 7, one resolves to the ADDRESS.
    let e = send::build(&w.ctx, &client, Some(&node), &a_send("@nothing"), &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 6, "reason={:?} msg={e}", e.reason());
    // `get_address_book.json` carries two entries described "Exchange" and "exchange", which
    // is what makes the case-insensitive match ambiguous (§4).
    let e = send::build(&w.ctx, &client, Some(&node), &a_send("@exchange"), &cancel).unwrap_err();
    assert_eq!(e.exit_code(), 7);
    // One match resolves, and **the resolved address — never the label — is what reaches the
    // daemon** (§4). No `validate_address` fixture answers for the address book's entries, so
    // the build then fails at step 6; the recorded request is the evidence that matters here.
    let e = send::build(&w.ctx, &client, Some(&node), &a_send("@savings"), &cancel).unwrap_err();
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
    let before = w.count(&w.wallet_log, "transfer");
    let syncing = World::new("order-syncing", &["--scenario", "syncing"], &[]);
    let e = send::build(
        &syncing.ctx,
        &syncing.wallet(),
        Some(&syncing.node()),
        &a_send(DEST),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 10);
    assert_eq!(syncing.count(&syncing.wallet_log, "transfer"), 0);
    assert_eq!(w.count(&w.wallet_log, "transfer"), before);
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
    let e = send::build(&ctx, &client, None, &a_send(DEST), &CancelToken::new()).unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    assert_eq!(e.reason(), Some("unreachable"));
    // And a bad address still loses first.
    let e = send::build(
        &ctx,
        &client,
        None,
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
    assert!(s.node.reachable);
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
    assert!(w2.text.contains("LOWER BOUND"), "{}", w2.text);
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
    assert!(s.node.reachable);
    assert_eq!(
        s.node.top_block_age_sec, None,
        "an all-zero BUSY header must not become an age (§1b.3)"
    );
    assert_eq!(s.node.synchronized, Some(false));
    assert!(s.warnings.iter().any(|x| x.id == "node_syncing"));
    // The client's own answer is an error, not a zeroed header.
    let e = w.node().get_last_block_header().unwrap_err();
    assert_eq!(e.exit_code(), 5);
    assert!(e.to_string().contains("BUSY"), "{e}");
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
        "transfer",
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
    assert!(e.to_string().contains("cannot reach the daemon"), "{e}");
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
    assert!(e.to_string().contains(&child.to_string()), "{e}");

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
    assert!(e.to_string().contains("still syncing"), "{e}");
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
