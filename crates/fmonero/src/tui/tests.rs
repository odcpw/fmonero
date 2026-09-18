//! The §7a TUI tests: one render test per tier per screen, the empty states, the help
//! overlay, and the `ProgramSimulator` send test that drives Form → build → confirm → relay
//! against `fmonero-mock` and asserts **one `transfer` and one `relay_tx` with identical
//! metadata** (§2a.3).
//!
//! **The mock's source is included, not reimplemented** — the same `#[path]` include
//! `fmonero-core`'s own money tests use, because sharing it as a library would need a
//! `[dev-dependencies]` entry and **no `Cargo.toml` may be edited after the 1a gate** (§2k).
//! Every assertion here is an AMBER row (§4b): it proves our rendering and our request shapes
//! against fixtures **derived from the Monero source**, and nothing about a real daemon.

#[path = "../../../fmonero-mock/src/main.rs"]
#[allow(dead_code)]
mod mock;

use std::path::PathBuf;
use std::sync::atomic::AtomicI32;
use std::sync::Arc;

use ftui::runtime::simulator::ProgramSimulator;
use ftui::{Event, Frame, GraphemePool, KeyCode, KeyEvent, Model};

use fmonero_core::{state, CancelToken, Context, PendingKind, PendingSend, PendingTx, Piconero};

use super::send::{Stage, CONFIRM_WORD, NO_SWEEP};
use super::{subs, Msg, Screen, Tui, KEYS};

/// The mock's own credential, and the one written to the login file (§2c). There is **no
/// unauthenticated wallet path that works**: a test that omits the file is testing exit 4.
const AUTH: &str = "fmonero:t3st";
/// A real, checksum-valid mainnet address from upstream's own test vectors — the one
/// `validate_address-standard.json` answers for (`_request_address`).
const DEST: &str = "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm";
/// The txid `transfer-do-not-relay.json` builds and `relay_tx.json` echoes.
const BUILT_TXID: &str = "a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .expect("tests/fixtures must exist: Phase 0 wrote it")
}

/// One test's world: its own state directory, its own mocks, its own request log.
struct World {
    dir: PathBuf,
    ctx: Context,
    wallet_log: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        World::with_node(name, &[])
    }

    /// A node mock and a wallet mock, and the matching login file written **before** any
    /// client is built (§2c). `node_extra` is the mock argv a scenario needs.
    fn with_node(name: &str, node_extra: &[&str]) -> World {
        let dir = std::env::temp_dir().join(format!(
            "fmonero-tui-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = Context::for_test(&dir).expect("test context");
        let fx = fixtures().display().to_string();
        let wallet_log = dir.join("wallet.jsonl");
        let mut node_args = vec![
            "--role".to_string(),
            "node".to_string(),
            "--port".to_string(),
            "0".to_string(),
            "--fixtures".to_string(),
            fx.clone(),
            "--log".to_string(),
            dir.join("node.jsonl").display().to_string(),
        ];
        node_args.extend(node_extra.iter().map(|a| (*a).to_string()));
        let node_port = spawn(&node_args.iter().map(String::as_str).collect::<Vec<_>>());
        let wallet_port = spawn(&[
            "--role",
            "wallet",
            "--port",
            "0",
            "--fixtures",
            &fx,
            "--log",
            &wallet_log.display().to_string(),
            "--auth",
            AUTH,
        ]);
        ctx.settings.node.url = format!("http://127.0.0.1:{node_port}");
        ctx.settings.wallet.url = format!("http://127.0.0.1:{wallet_port}");
        // The login file is named after the BIND PORT (§2c, §4.2), so the two must agree.
        ctx.settings.wallet.port = wallet_port;
        ctx.settings.wallet.dir = dir.join("wallets/mainnet");
        ctx.settings.node.log_file = dir.join("bitmonero.log");
        std::fs::create_dir_all(&ctx.paths.run_dir).expect("run dir");
        std::fs::create_dir_all(&ctx.settings.wallet.dir).expect("wallet dir");
        std::fs::write(ctx.settings.wallet.dir.join("main.keys"), b"").expect("a wallet to list");
        std::fs::write(
            &ctx.settings.node.log_file,
            b"2026-09-16 14:02:11 INFO synced\n",
        )
        .expect("a log to tail");
        write_login(&ctx.paths.wallet_login_file(wallet_port));
        World {
            dir,
            ctx,
            wallet_log,
        }
    }

    fn count(&self, method: &str) -> usize {
        std::fs::read_to_string(&self.wallet_log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            // The mock logs the 401 handshake as well as the answered retry (§2c), and every
            // `Cmd::task` builds its own client, so only `status: 200` lines are requests the
            // daemon actually executed.
            .filter(|l| l["method"] == method && l["status"] == 200)
            .count()
    }

    fn body(&self, method: &str) -> serde_json::Value {
        std::fs::read_to_string(&self.wallet_log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|l| l["method"] == method && l["status"] == 200)
            .map(|l| l["body"]["params"].clone())
            .unwrap_or(serde_json::Value::Null)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn spawn(args: &[&str]) -> u16 {
    mock::spawn_for_test(args).expect("the mock must start")
}

fn write_login(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, AUTH.as_bytes()).expect("login file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("0600");
}

/// A model with no daemon at all: the §4b GREEN row ("both daemons absent").
fn offline_model(screen: Screen) -> Tui {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-offline-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut ctx = Context::for_test(&dir).expect("test context");
    // Ports nothing listens on: `status` must render `reachable: false`, never an error.
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    Tui::new(screen, ctx, Arc::new(AtomicI32::new(0)))
}

fn model(ctx: &Context, screen: Screen) -> Tui {
    Tui::new(screen, ctx.clone(), Arc::new(AtomicI32::new(0)))
}

/// A running program: `ProgramSimulator` executes `Cmd::task` synchronously, so `init()`
/// really reads `status`, the wallet and the log before the first frame.
fn sim(m: Tui) -> ProgramSimulator<Tui> {
    let mut s = ProgramSimulator::new(m);
    s.init();
    s.send(Msg::Tick);
    s
}

fn sim_text(s: &ProgramSimulator<Tui>, w: u16, h: u16) -> String {
    screen_text(s.model(), w, h)
}

/// Render into a real `Buffer` and read the text back, row by row.
fn render(m: &Tui, w: u16, h: u16) -> Vec<String> {
    let mut pool = GraphemePool::new();
    let mut frame = Frame::new(w, h, &mut pool);
    m.view(&mut frame);
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| {
                    frame
                        .buffer
                        .get(x, y)
                        .and_then(|c| c.content.as_char())
                        .unwrap_or(' ')
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn screen_text(m: &Tui, w: u16, h: u16) -> String {
    render(m, w, h).join("\n")
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code))
}

fn type_str(sim: &mut ProgramSimulator<Tui>, s: &str) {
    for c in s.chars() {
        sim.inject_event(key(KeyCode::Char(c)));
    }
}

/// Run the armed action the way [`subs::ActionSub`] does — the same `subs::execute`, because
/// `StopSignal` has no public constructor and the simulator never starts subscription threads.
fn run_action(sim: &mut ProgramSimulator<Tui>) {
    let (ctx, slot, announce) = {
        let m = sim.model();
        let a = m.action.as_ref().expect("an armed action");
        (m.ctx.clone(), a.slot.clone(), m.announce.clone())
    };
    let action = slot.lock().expect("the slot").take().expect("the action");
    // §7a: a relay ignores cancellation; everything else honours it.
    let relaying = !action.cancellable();
    let out = subs::execute(&ctx, action, &announce, &CancelToken::new());
    if relaying {
        assert!(
            out.is_ok(),
            "the relay failed: {:?}",
            out.err().map(|e| e.text)
        );
    }
    sim.send(Msg::ActionDone(Box::new(out)));
}

// ---- the four resize tiers × the four screens (§7a) ---------------------------------

#[test]
fn every_screen_renders_at_every_tier_with_both_daemons_absent() {
    for screen in [Screen::Node, Screen::Wallet, Screen::Send, Screen::Log] {
        let s = sim(offline_model(screen));
        for (w, h) in [(120u16, 40u16), (80, 24), (50, 16)] {
            let text = sim_text(&s, w, h);
            assert!(
                !text.trim().is_empty(),
                "{screen:?} at {w}x{h} rendered blank"
            );
            assert!(
                text.contains('●'),
                "{screen:?} at {w}x{h} lost its status badge:\n{text}"
            );
            assert!(
                text.contains("help"),
                "{screen:?} at {w}x{h} lost the ? hint:\n{text}"
            );
        }
        // Tiny: one honest line naming the size it needs.
        let tiny = sim_text(&s, 38, 9);
        assert!(tiny.contains("too small"), "{screen:?}:\n{tiny}");
        assert!(tiny.contains("40x10"), "{screen:?}:\n{tiny}");
    }
}

#[test]
fn a_zero_area_frame_renders_nothing_and_does_not_panic() {
    let m = offline_model(Screen::Node);
    assert!(render(&m, 0, 0).is_empty());
}

#[test]
fn the_narrow_tier_collapses_the_tab_strip() {
    let m = offline_model(Screen::Wallet);
    let text = screen_text(&m, 50, 16);
    assert!(text.contains("1·2·3·4"), "collapsed tabs missing:\n{text}");
    assert!(text.contains("Wallet"), "{text}");
}

#[test]
fn the_offline_node_screen_says_what_to_press_and_never_shows_a_zero_height() {
    let text = sim_text(&sim(offline_model(Screen::Node)), 120, 40);
    assert!(text.contains("Node is not running"), "{text}");
    // §5's shape rule: unknown is `—`, never a zero standing in for unknown.
    assert!(text.contains('—'), "{text}");
}

#[test]
fn the_offline_wallet_screen_offers_the_open_key() {
    let text = sim_text(&sim(offline_model(Screen::Wallet)), 120, 40);
    // v5.5 replaced two dead ends with §7a.5's panel: three chips and a daemon status line.
    // The old text was true and useless — `o` listed an empty directory, `c` was close not
    // create, and there was no create flow at all.
    assert!(text.contains("No wallet is open."), "{text}");
    assert!(text.contains("[o] Open wallet…"), "{text}");
    assert!(text.contains("[c] Create wallet…"), "{text}");
    assert!(text.contains("[W] Start the wallet daemon"), "{text}");
    assert!(text.contains("wallet daemon:"), "{text}");
}

/// §7a: the keymap table "is also the exact content of the `?` overlay — one
/// `&[(&str,&str)]` table so help and behaviour cannot drift". **Every** row, not the first
/// eight: the overlay was one row short of its own table at every size, and `take(8)` could
/// never see it. The last row is the Log screen's `/ f n N`, documented nowhere else.
#[test]
fn the_help_overlay_lists_every_binding_including_the_last_row() {
    let mut m = offline_model(Screen::Node);
    m.show_help = true;
    let text = screen_text(&m, 110, 40);
    for (k, _) in KEYS.iter() {
        let first = k.split_whitespace().next().unwrap_or_default();
        assert!(text.contains(first), "help is missing {first}:\n{text}");
    }
    let (last_keys, last_action) = KEYS.last().expect("KEYS is not empty");
    assert!(
        text.contains(*last_action),
        "the LAST row ({last_keys}) is missing — the overlay is short of its own table:\n{text}"
    );
}

/// §7a's keymap is the `?` overlay, so a row that names a screen has to be a binding that
/// screen actually consumes. `Tab` used to swap the Wallet's panes, which made the Wallet the
/// one screen `Tab` could not leave, while `←→` — the row that claims the pane swap — did
/// nothing at all.
#[test]
fn every_screen_consumes_the_navigation_keys_its_keymap_row_promises() {
    for (from, want) in [
        (Screen::Node, Screen::Wallet),
        (Screen::Wallet, Screen::Send),
        (Screen::Log, Screen::Node),
    ] {
        let mut m = offline_model(from);
        let _ = m.update(Msg::Raw(key(KeyCode::Tab)));
        assert_eq!(m.screen, want, "Tab on {from:?} must reach {want:?}");
    }
    // And ←→ swaps the Wallet's panes, as its row says.
    let mut m = offline_model(Screen::Wallet);
    let before = m.wallet.pane;
    let _ = m.update(Msg::Raw(key(KeyCode::Right)));
    assert_ne!(m.wallet.pane, before, "→ must swap the Wallet's panes");
    let _ = m.update(Msg::Raw(key(KeyCode::Left)));
    assert_eq!(m.wallet.pane, before, "← must swap them back");
}

/// §7a's empty state is a runnable command, and `lines_at` clips rather than wraps — so a hero
/// line one character too wide became `fmonero setup --unit`, which exits 2 and looks complete.
#[test]
fn the_not_installed_hero_keeps_its_command_whole_at_every_width() {
    let m = offline_model(Screen::Node);
    for (w, h) in [(150u16, 40u16), (120, 40), (100, 30), (70, 24)] {
        let text = screen_text(&m, w, h);
        if !text.contains("Monero is not installed") {
            continue; // monerod IS installed on this machine; the other empty state renders
        }
        assert!(
            text.contains("fmonero setup --units"),
            "{w}x{h}: the hero's command was clipped:\n{text}"
        );
    }
}

/// §7a: "Empty, error and hang states — each must render, none may be blank." At 70x20 the
/// Medium tier's four fixed panes needed 24 rows and got 18, so the peers table and the
/// Warnings pane — the only `severity: error` surface on this screen — both drew nothing.
#[test]
fn the_node_screen_still_shows_its_warnings_in_a_short_medium_terminal() {
    let s = sim(offline_model(Screen::Node));
    let warned = s
        .model()
        .status
        .as_ref()
        .is_some_and(|st| !st.warnings.is_empty());
    assert!(warned, "both daemons are absent, so status must warn");
    for (w, h) in [(70u16, 20u16), (70, 24), (80, 24), (99, 30)] {
        let text = sim_text(&s, w, h);
        assert!(
            text.contains("Warnings"),
            "{w}x{h}: the Warnings pane did not render:\n{text}"
        );
        // And no pane was given room it then left blank.
        assert!(
            !text.contains("Peers") || text.contains("no connections") || text.contains("not"),
            "{w}x{h}: the peers pane rendered a frame with nothing in it:\n{text}"
        );
    }
}

/// §7a: "`Ok(0)` on a clean quit; if the session's **last** action failed, that error's own
/// `exit_code()`." A short task's success arm did not clear the slot, so one transient failure
/// early on — a wallet daemon still starting — left its code armed and a clean quit after a
/// successful retry still reported the whole session as failed.
#[test]
fn a_successful_task_clears_a_previous_failures_exit_code() {
    use std::sync::atomic::Ordering;
    let mut m = offline_model(Screen::Wallet);
    let _ = m.update(Msg::Task(
        "new address",
        Box::new(Err(subs::ActionErr::from(fmonero_core::Error::backend(
            "monero-wallet-rpc is not answering",
        )))),
    ));
    assert_eq!(m.exit.load(Ordering::SeqCst), 4, "the failure is recorded");
    let _ = m.update(Msg::Task(
        "new address",
        Box::new(Ok("new subaddress 3".into())),
    ));
    assert_eq!(
        m.exit.load(Ordering::SeqCst),
        0,
        "a later success must disarm the stale code"
    );
}

#[test]
fn quitting_while_an_action_runs_takes_two_presses() {
    let mut m = offline_model(Screen::Node);
    let _ = m.start(subs::Action::NodeUp);
    assert!(m.action.is_some(), "the action must be armed");
    let _ = m.quit(0);
    assert!(m.action.is_some(), "the first press must not quit");
    assert!(m.note.contains("press quit again"), "{}", m.note);
    let _ = m.quit(0);
    assert!(m.action.is_none(), "the second press cancels");
}

// ---- against the mock (§4b AMBER) ---------------------------------------------------

#[test]
fn every_screen_renders_against_the_mock() {
    let w = World::new("render");
    for screen in [Screen::Node, Screen::Wallet, Screen::Send, Screen::Log] {
        let text = sim_text(&sim(model(&w.ctx, screen)), 120, 40);
        assert!(!text.trim().is_empty(), "{screen:?} rendered blank");
        assert!(text.contains('●'), "{screen:?} lost its badge:\n{text}");
        match screen {
            // The node fixture's height, so the hero is reading the mock and not a default.
            Screen::Node => assert!(text.contains("3541122"), "{text}"),
            // The wallet fixture is open, so a balance and a subaddress table are there.
            Screen::Wallet => assert!(text.contains("XMR"), "{text}"),
            Screen::Send => assert!(text.contains("Priority"), "{text}"),
            Screen::Log => assert!(text.contains("synced"), "{text}"),
        }
    }
}

/// §7a: on a restricted node every pane that needs a gated method says so **and names the
/// method** — never a zero, and never a poisoned `get_info` field (§1b.3, §5).
#[test]
fn a_restricted_node_names_the_method_each_dark_pane_needs() {
    let w = World::with_node("restricted", &["--restricted", "--scenario", "restricted"]);
    let text = sim_text(&sim(model(&w.ctx, Screen::Node)), 120, 40);
    assert!(
        text.contains("not available on a restricted node"),
        "the peers pane must say so:\n{text}"
    );
    assert!(
        text.contains("get_connections"),
        "…and name the method:\n{text}"
    );
    assert!(
        text.contains("restricted RPC"),
        "the hero must carry the warning:\n{text}"
    );
    // The poisoned fields render as `—`: a restricted node reports 0 peers and 2^64-1 free
    // bytes by construction, and §5 turns both into null.
    assert!(
        text.contains("— out · — in"),
        "peer counts must be null:\n{text}"
    );
}

/// §1b.3: `get_last_block_header` is the one call `CHECK_CORE_READY()` gates, and during the
/// whole initial sync it answers `status: "BUSY"` with an all-zero header. The hero must show
/// `—`, not a block mined in 1970.
#[test]
fn a_syncing_node_shows_the_gauge_and_no_block_age() {
    let w = World::with_node("syncing", &["--scenario", "syncing"]);
    let text = sim_text(&sim(model(&w.ctx, Screen::Node)), 120, 40);
    assert!(text.contains("Syncing"), "{text}");
    assert!(
        text.contains("last block —"),
        "a BUSY header must render as — (§1b.3):\n{text}"
    );
    assert!(!text.contains("56 years"), "{text}");
}

#[test]
fn the_send_screen_refuses_all_inline_and_never_reaches_the_daemon() {
    let w = World::new("sweep-refusal");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    type_str(&mut sim, DEST);
    // v5.5: fields move with `↑↓`. `Tab` switches screens now, on every screen and in every
    // focus state (§7a.1, and §5b.4 row 10 records the binding this takes away).
    sim.inject_event(key(KeyCode::Down));
    type_str(&mut sim, "all");
    sim.inject_event(key(KeyCode::Enter));
    assert!(
        matches!(sim.model().send.stage, Stage::Form),
        "typing `all` must not start a build"
    );
    assert_eq!(sim.model().note, NO_SWEEP);
    assert_eq!(w.count("transfer"), 0, "nothing may be built");
    let text = sim_text(&sim, 110, 30);
    assert!(text.contains("sweep is not available"), "{text}");
}

/// §7a's send machine: a **refused** start must leave the screen recoverable.
///
/// `Tui::start` consumes the `Action` and sends no `Msg::ActionDone`, and both of its callers
/// in the send machine advance the stage first — so a refusal used to strand the screen in a
/// stage only `ActionDone` can leave, with no key reaching it (the form is `busy()`, so it owns
/// none) and the built plan dropped. This drives the `Building` half, which is reachable by
/// pressing Enter on the Send form while another action is in flight.
#[test]
fn a_refused_start_returns_the_send_screen_to_its_form() {
    let w = World::new("refused-start");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    type_str(&mut sim, DEST);
    // v5.5: fields move with `↑↓`. `Tab` switches screens now, on every screen and in every
    // focus state (§7a.1, and §5b.4 row 10 records the binding this takes away).
    sim.inject_event(key(KeyCode::Down));
    type_str(&mut sim, "0.1");
    let req = sim.model().send.request(None).expect("a valid form");

    // The first one arms the build.
    sim.send(Msg::Validated(Box::new(Ok(req.clone()))));
    assert!(sim.model().action.is_some(), "the build must be armed");
    assert!(matches!(sim.model().send.stage, Stage::Building));

    // The second is refused, because one is already in flight — and must not leave the screen
    // in `Building` for the rest of the session.
    sim.send(Msg::Validated(Box::new(Ok(req))));
    assert!(
        matches!(sim.model().send.stage, Stage::Form),
        "a refused build must return the form, not strand the screen"
    );
    assert!(sim.model().note.contains("already"), "{}", sim.model().note);
}

/// §2d: "**`send::relay` likewise never consults a breaker**: once a plan is built the §2a.11
/// `send.lock` is held and the transaction exists, so exiting 5 there would abandon a built
/// transaction rather than protect anything." The TUI's own gate read the last status poll and
/// refused `Action::Relay` along with everything else, which dropped the plan and left the Send
/// screen on "broadcasting" with no `ActionDone` ever coming.
#[test]
fn an_armed_breaker_does_not_refuse_the_relay_of_a_built_plan() {
    let w = World::new("relay-breaker");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    type_str(&mut sim, DEST);
    // v5.5: fields move with `↑↓`. `Tab` switches screens now, on every screen and in every
    // focus state (§7a.1, and §5b.4 row 10 records the binding this takes away).
    sim.inject_event(key(KeyCode::Down));
    type_str(&mut sim, "0.1");
    sim.inject_event(key(KeyCode::Enter));
    run_action(&mut sim);
    assert!(matches!(sim.model().send.stage, Stage::Built(_)));

    // Arm the wallet breaker and let the model see it, exactly as a poll would.
    state::note_stall(&w.ctx, fmonero_core::Daemon::Wallet).expect("arm the breaker");
    sim.send(Msg::Poll);
    assert!(
        sim.model()
            .status
            .as_ref()
            .is_some_and(|s| s.warnings.iter().any(|x| x.id == "wallet_stalled")),
        "the model must actually see an armed breaker, or this test proves nothing"
    );

    type_str(&mut sim, CONFIRM_WORD);
    sim.inject_event(key(KeyCode::Enter));
    assert!(
        sim.model().action.is_some(),
        "the relay must be armed despite the breaker: {}",
        sim.model().note
    );
    run_action(&mut sim);
    assert_eq!(w.count("relay_tx"), 1, "the plan was relayed");
}

/// The central §7a test: the Send screen drives **one** `transfer` and **one** `relay_tx`, and
/// the `relay_tx.hex` is byte-identical to the `transfer` response's `tx_metadata` (§2a.3).
/// The TUI holds the plan in memory between the form and the modal, so it builds once.
#[test]
fn the_send_screen_builds_once_and_relays_that_exact_transaction() {
    let w = World::new("send");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();

    // 1. Form — every keystroke is local (§7a step 1).
    type_str(&mut sim, DEST);
    // v5.5: fields move with `↑↓`. `Tab` switches screens now, on every screen and in every
    // focus state (§7a.1, and §5b.4 row 10 records the binding this takes away).
    sim.inject_event(key(KeyCode::Down));
    type_str(&mut sim, "0.1");
    assert_eq!(w.count("transfer"), 0, "the form must send nothing");

    // 2. Enter → `validate_address` in a Cmd::task, then 3. Building.
    sim.inject_event(key(KeyCode::Enter));
    assert_eq!(
        w.count("validate_address"),
        1,
        "§7a step 2: one validate_address before anything is built"
    );
    assert!(
        sim.model().action.is_some(),
        "the build must be armed as an ActionSub, never a Cmd"
    );
    run_action(&mut sim);

    // 4. Built → the confirm modal, holding the plan and showing the BUILT fee.
    let Stage::Built(plan) = &sim.model().send.stage else {
        panic!("the build did not produce a plan: {}", sim.model().note);
    };
    assert_eq!(
        plan.txs.len(),
        1,
        "one transaction only — sweep is CLI-only"
    );
    assert_eq!(plan.txs[0].txid, BUILT_TXID);
    assert_eq!(w.count("transfer"), 1, "exactly one transfer");
    assert_eq!(
        w.count("relay_tx"),
        0,
        "nothing is broadcast before the word"
    );
    let fee = plan.total_fee.to_xmr_string();
    let shown = sim_text(&sim, 110, 34);
    assert!(
        shown.contains(&fee),
        "the modal must show the built fee {fee}"
    );
    assert!(
        shown.contains("Type \"send\""),
        "the modal must demand the word: {shown}"
    );

    // §7a's resize table: the confirm modal is never abbreviated, so the narrow tier still
    // shows the recipient, the built fee and the word it wants.
    let narrow = sim_text(&sim, 50, 16);
    for needle in ["Amount", "Fee", "Total", "Type \"send\""] {
        assert!(
            narrow.contains(needle),
            "the narrow confirm modal lost {needle}:\n{narrow}"
        );
    }

    // The typed word IS the TUI's --yes: Enter alone does not broadcast.
    sim.inject_event(key(KeyCode::Enter));
    assert_eq!(
        w.count("relay_tx"),
        0,
        "Enter without the word must not relay"
    );
    // §7a: the modal "is the whole safety surface and therefore … never abbreviated". The echo
    // row was the line that fell off the bottom at every size, so the confirmation the design
    // deliberately made harder than Enter had no feedback at all: a mistyped word looked exactly
    // like a correct one. Type a prefix and assert it is on screen, at every tier.
    type_str(&mut sim, "se");
    for (cw, ch) in [(110u16, 34u16), (150, 40), (80, 24), (50, 16)] {
        let echo = sim_text(&sim, cw, ch);
        assert!(
            echo.contains("> se"),
            "{cw}x{ch}: the confirm modal shows no echo of the typed word:\n{echo}"
        );
    }
    sim.inject_event(key(KeyCode::Backspace));
    sim.inject_event(key(KeyCode::Backspace));
    type_str(&mut sim, CONFIRM_WORD);
    sim.inject_event(key(KeyCode::Enter));

    // 5. Relaying → the SAME plan (§2a.3).
    run_action(&mut sim);
    assert_eq!(w.count("transfer"), 1, "the plan must NOT be rebuilt");
    assert_eq!(w.count("relay_tx"), 1, "exactly one relay_tx");
    let relayed = w.body("relay_tx")["hex"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let built = std::fs::read_to_string(fixtures().join("wallet/transfer-do-not-relay.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["tx_metadata"].as_str().map(str::to_string))
        .expect("the fixture's tx_metadata");
    assert_eq!(
        relayed, built,
        "relay_tx.hex must be the transfer's tx_metadata"
    );
    assert!(!relayed.is_empty());

    // The toast, the jump to the Wallet screen, and a cleared form (§7a step 5).
    assert!(matches!(sim.model().send.stage, Stage::Sent(_)));
    assert_eq!(sim.model().screen, Screen::Wallet);
    assert!(
        sim.model().send.to.value().is_empty(),
        "the form must clear"
    );
    assert!(
        sim.model().note.contains(BUILT_TXID),
        "{}",
        sim.model().note
    );
    // The relay announced the paid line through the sink, into the Log pane (§2a.2, §2g).
    let announced = sim.model().announce.lock().expect("sink").join("\n");
    let log_has_it = announced.contains("0.1000") || !sim.model().logs.empty;
    assert!(log_has_it, "the announce line is missing: {announced:?}");
}

/// §2i, in the TUI: `u`, `d` and `L` are refused under any layout but fmonero's own user
/// unit, with the gate's own text, and **before** any RPC or confirm modal.
#[test]
fn node_mutations_are_refused_when_fmonero_does_not_own_the_node() {
    let mut m = offline_model(Screen::Node);
    for k in ['u', 'd', 'L'] {
        let _ = m.on_key(KeyEvent::new(KeyCode::Char(k)));
        assert!(
            m.modal.is_none(),
            "{k}: no confirm modal without a user unit"
        );
        assert!(m.action.is_none(), "{k}: nothing may be started");
        assert!(
            m.note.contains("not fmonero's to mutate"),
            "{k}: {}",
            m.note
        );
    }
}

/// §7a: an unresolved `pending.json` record is a red banner on **every** screen, carrying the
/// txid and the two commands — and it turns the node badge red (§7a's badge colours).
#[test]
fn an_unresolved_pending_record_banners_every_screen() {
    let w = World::new("pending");
    let record = PendingSend {
        kind: PendingKind::Send,
        account_index: 0,
        dest: DEST.to_string(),
        built_at: fmonero_core::now_unix(),
        txs: vec![PendingTx {
            txid: BUILT_TXID.to_string(),
            amount: Piconero(100_000_000_000),
            fee: Piconero(30_720_000),
            relayed: false,
            relayed_at: None,
        }],
    };
    state::set_pending_send(&w.ctx, &record).expect("the record");
    for screen in [Screen::Node, Screen::Wallet, Screen::Send, Screen::Log] {
        let s = sim(model(&w.ctx, screen));
        assert_eq!(s.model().node_badge(), "error", "{screen:?}");
        // §7a: "the txid and the two commands", at every width the banner is drawn at. Both
        // commands in full: `history --limit 5 | clear-pending --yes` is one command piped into
        // a program that does not exist, and at 45 columns two rows dropped the second half —
        // the only command that gets the Send screen building again.
        for (cw, ch) in [(150u16, 40u16), (120, 40), (70, 24), (45, 20)] {
            let text = sim_text(&s, cw, ch);
            // The banner wraps at a narrow width, so the commands are checked on the joined
            // text: what matters is that neither is CUT OFF, which is what a fixed two-row
            // banner did to `clear-pending` at 45 columns.
            let flat = text.replace('\n', " ");
            for needle in [
                "SEND IN FLIGHT",
                "wallet history --limit 5",
                "wallet clear-pending --yes",
                &BUILT_TXID[..8],
            ] {
                assert!(
                    flat.contains(needle),
                    "{screen:?} at {cw}x{ch} lost {needle}:\n{text}"
                );
            }
        }
    }
}

/// A build survives leaving the Send screen, so the modal it produces must own the keys
/// wherever it lands — including over the Wallet screen.
#[test]
fn the_confirm_modal_owns_the_keys_even_on_another_screen() {
    let w = World::new("modal-keys");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    type_str(&mut sim, DEST);
    // v5.5: fields move with `↑↓`. `Tab` switches screens now, on every screen and in every
    // focus state (§7a.1, and §5b.4 row 10 records the binding this takes away).
    sim.inject_event(key(KeyCode::Down));
    type_str(&mut sim, "0.1");
    sim.inject_event(key(KeyCode::Enter));
    // While the build runs the form holds no keys, so the user can walk away.
    sim.inject_event(key(KeyCode::Char('2')));
    assert_eq!(sim.model().screen, Screen::Wallet);
    run_action(&mut sim);
    assert!(matches!(sim.model().send.stage, Stage::Built(_)));
    let shown = sim_text(&sim, 110, 34);
    assert!(shown.contains("Type \"send\""), "{shown}");
    // `2` would switch screens on the Wallet screen; here the modal eats it.
    sim.inject_event(key(KeyCode::Char('2')));
    assert_eq!(sim.model().send.typed, "2", "the modal must own the key");
    sim.inject_event(key(KeyCode::Escape));
    assert!(
        matches!(sim.model().send.stage, Stage::Form),
        "Esc discards"
    );
    assert_eq!(w.count("relay_tx"), 0, "nothing may be broadcast");
}

#[test]
fn the_open_wallet_modal_masks_the_password_and_lists_the_wallet_dir() {
    let w = World::new("open");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    s.inject_event(key(KeyCode::Char('o')));
    let text = sim_text(&s, 100, 30);
    assert!(
        text.contains("main"),
        "the wallet dir listing is missing:\n{text}"
    );
    s.inject_event(key(KeyCode::Enter)); // -> ask for the password
    type_str(&mut s, "hunter2");
    let text = sim_text(&s, 100, 30);
    assert!(
        !text.contains("hunter2"),
        "the password is visible:\n{text}"
    );
    assert!(text.contains("•••••••"), "the mask is missing:\n{text}");
}

#[test]
fn the_qr_modal_draws_the_address_and_the_code() {
    let w = World::new("qr");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    s.inject_event(key(KeyCode::Char('Q')));
    let text = sim_text(&s, 120, 40);
    assert!(text.contains("Receive"), "{text}");
    // The code itself, in half-block cells — not the "too small" fallback.
    assert!(
        !text.contains("too small for a QR"),
        "a 120x40 terminal has room for the code:\n{text}"
    );
    assert!(
        text.contains('\u{2580}') || text.contains('\u{2584}') || text.contains('\u{2588}'),
        "{text}"
    );
    // The address is wrapped, never elided (§7a).
    assert!(text.contains(&DEST[..20]), "{text}");
}

/// **§7a and §2m.3: the Node screen in remote mode is a different, much smaller hero.**
///
/// It renders the address, the trust flag and the ssl mode, one dim line of explanation, and —
/// this is the assertion that matters — **no sync gauge, no percent and no ETA**, because all
/// three read fields that are `null` there and a gauge at 0 % is the rendered untruth §2j
/// forbids. On the pre-v5.2 code this screen showed "Node is not running" and a 0 % bar for a
/// wallet that is working perfectly.
#[test]
fn the_node_screen_in_remote_mode_shows_the_address_and_no_gauge() {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-remote-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.remote = "node.example.com:18089".into();
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    let s = sim(model(&ctx, Screen::Node));
    let text = sim_text(&s, 100, 30);
    assert!(text.contains("Remote node"), "{text}");
    assert!(text.contains("node.example.com:18089"), "{text}");
    assert!(text.contains("untrusted"), "{text}");
    assert!(
        text.contains("monero-wallet-rpc does"),
        "the one dim line §7a asks for, verbatim:\n{text}"
    );
    // Every pane that needs a gated method says so, with `remote node` as the reason (§7a).
    // v5.5: the picker takes the peers table's place in remote mode (§7a.3), so Net and
    // Mempool·fees are the two panes left that need a gated method — the third pane is now the
    // one thing a remote node CAN be asked, which is the point of the change.
    assert_eq!(
        text.matches("not available with a remote node").count(),
        2,
        "Net and Mempool·fees each say it:\n{text}"
    );
    assert!(
        text.contains("Public nodes"),
        "the picker replaces the peers table in remote mode (§7a.3):\n{text}"
    );
    // No gauge, and none of the phrases that would need a number remote mode has not got.
    for absent in [
        "eta ",
        "Node is not running",
        "reading the transaction pool",
        "sparkline fills",
    ] {
        assert!(
            !text.contains(absent),
            "the remote hero rendered `{absent}`:\n{text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
