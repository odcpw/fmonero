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
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use ftui::runtime::simulator::ProgramSimulator;
use ftui::{Event, Frame, GraphemePool, KeyCode, KeyEvent, Model, Modifiers};

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

    /// The same, with argv for the **wallet** mock — `--scenario behind` is the one case that
    /// needs it, and it is the state decision 7 exists for (v5.6.1, V3).
    fn with_wallet(name: &str, wallet_extra: &[&str]) -> World {
        World::with_mocks(name, &[], wallet_extra)
    }

    /// A node mock and a wallet mock, and the matching login file written **before** any
    /// client is built (§2c). `node_extra` is the mock argv a scenario needs.
    fn with_node(name: &str, node_extra: &[&str]) -> World {
        World::with_mocks(name, node_extra, &[])
    }

    fn with_mocks(name: &str, node_extra: &[&str], wallet_extra: &[&str]) -> World {
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
        let mut wallet_args = vec![
            "--role".to_string(),
            "wallet".to_string(),
            "--port".to_string(),
            "0".to_string(),
            "--fixtures".to_string(),
            fx.clone(),
            "--log".to_string(),
            wallet_log.display().to_string(),
            "--auth".to_string(),
            AUTH.to_string(),
        ];
        wallet_args.extend(wallet_extra.iter().map(|a| (*a).to_string()));
        let wallet_port = spawn(&wallet_args.iter().map(String::as_str).collect::<Vec<_>>());
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

/// The screen's text with every line break and every run of spaces collapsed to one space, so a
/// sentence that **wrapped** — which is what v5.6.2 does instead of eliding — can be asserted
/// whole. A frame's rows are 120 cells wide and padded; `contains` on the raw text can only ever
/// see a sentence that happens to fit one row.
fn flowed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
        // Flattened, because the sentence wraps at 38 columns — which is the whole point of
        // the state it describes.
        let tiny = sim_text(&s, 38, 9).replace('\n', " ");
        assert!(tiny.contains("too small"), "{screen:?}:\n{tiny}");
        assert!(tiny.contains("40 by 10"), "{screen:?}:\n{tiny}");
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
    assert!(text.contains("Your node is not running"), "{text}");
    // **Round 5, S10: with nothing set up, the action is the one that sets it up.** This asserted
    // `Press u to start it` on a model whose layout is `none` — and `u` there answered *"Another
    // program on this machine runs this node"*, about a node that does not exist. The `u` case is
    // asserted below, on a model that has the unit file `u` needs.
    assert!(
        flowed(&text.replace('│', " "))
            .contains("fmonero has not set up your node yet. Press S to set it up."),
        "{text}"
    );
    // **X23: and not a row of dashes under it.** §5's rule is that unknown is `—` and never a zero;
    // with the node off there is no measurement to label at all, so the three rows that read
    // `last block —` / `peers — out · — in` / `version — · —` are not drawn, and the hero's own
    // sentence is the whole answer. The shape §6.5 deleted from the widget and W3 from `status`.
    for gone in ["last block", "— out · — in"] {
        assert!(
            !text.contains(gone),
            "with the node off, `{gone}` carries nothing:\n{text}"
        );
    }
    // And the node's own state is stated **once** in the frame, not in every pane that has no
    // answer: it filled the hero, the badge, the Net pane, the waiting pane, the Peers pane and a
    // Notes bullet — six times. The badge is the statement; the hero's second row is the action;
    // each pane says what is missing from IT. (The wallet service's own row is a different fact and
    // is allowed to say so.)
    assert_eq!(
        text.matches("Your node is not running").count(),
        1,
        "one statement per frame (X23):\n{text}"
    );
    assert!(
        text.contains("No traffic while your node is off.")
            && text.contains("No peers while your node is off.")
            && text.contains("Nothing waiting while your node is off."),
        "each pane says what is missing from it (X23):\n{text}"
    );
}

#[test]
fn the_offline_wallet_screen_offers_the_open_key() {
    let text = sim_text(&sim(offline_model(Screen::Wallet)), 120, 40);
    // v5.5 replaced two dead ends with §7a.5's panel: three chips and a daemon status line.
    // The old text was true and useless — `o` listed an empty directory, `c` was close not
    // create, and there was no create flow at all.
    assert!(text.contains("No wallet is open."), "{text}");
    // v5.6.1, V12: `Open wallet…      choose one of the 0 in /home/…` became a sentence, and the
    // balance pane that rendered `— XMR   unlocked — XMR` above this panel is gone with it.
    assert!(!text.contains("unlocked — XMR"), "{text}");
    assert!(text.contains("[c] Create a wallet"), "{text}");
    assert!(text.contains("wallet service:"), "{text}");
    // **Only the chips that can act** (round 4 iteration 3). With no wallets on this machine `o`
    // has nothing to open and its label said so — `(none on this machine yet. Press c to create
    // one)` — and `W` said `(not needed: opening a wallet starts it)`. Two controls telling the
    // reader not to press them, on the screen whose whole content is controls. `Create a wallet`
    // is the one thing to do here, and both keys are still in the `?` overlay.
    assert!(
        !text.contains("[o] Open a wallet"),
        "with no wallets there is nothing for o to open:\n{text}"
    );
    assert!(
        !text.contains("[W] Start the wallet service"),
        "opening a wallet starts the service, so this chip has nothing to offer:\n{text}"
    );
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
        // §2o.1's first exception, and the only command left on this hero: installing a package
        // is not something fmonero does (docs/COPY.md §2). It must still be whole.
        assert!(
            text.contains("omarchy pkg add monero"),
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
            text.contains("Notes"),
            "{w}x{h}: the notes pane did not render:\n{text}"
        );
        // And no pane was given room it then left blank.
        assert!(
            !text.contains("Peers") || text.contains("No peers yet") || text.contains("not"),
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
            "The wallet service is not answering.",
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
            // The node fixture's height, so the hero is reading the mock and not a default —
            // grouped, because every block number on every surface is (W7).
            Screen::Node => assert!(text.contains("3 541 122"), "{text}"),
            // The wallet fixture is open, so a balance and a subaddress table are there.
            Screen::Wallet => assert!(text.contains("XMR"), "{text}"),
            Screen::Send => assert!(text.contains("cheapest"), "{text}"),
            Screen::Log => assert!(text.contains("synced"), "{text}"),
        }
    }
}

/// §7a: on a restricted node every pane that needs a gated method says so — never a zero, and
/// never a poisoned `get_info` field (§1b.3, §5). **v5.6 drops the method name from the
/// sentence** (§2o rule 5: an RPC method is an internal identifier).
#[test]
fn a_restricted_node_says_each_dark_pane_is_unanswered() {
    let w = World::with_node("restricted", &["--restricted", "--scenario", "restricted"]);
    let text = sim_text(&sim(model(&w.ctx, Screen::Node)), 120, 40);
    assert!(
        text.contains("This node does not answer that."),
        "the peers pane must say so:\n{text}"
    );
    // **And never the method name** — that is the point of the rewrite (§2o rule 5).
    assert!(
        !text.contains("get_connections"),
        "no RPC method may be shown as prose:\n{text}"
    );
    assert!(
        text.contains("only answers basic questions"),
        "the hero must carry the note:\n{text}"
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
        text.contains("last block  —"),
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
    assert!(text.contains("Sweep is not in the TUI yet"), "{text}");
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
    assert!(sim.model().note.contains("Already"), "{}", sim.model().note);
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
    assert!(matches!(sim.model().send.stage, Stage::Sent));
    assert_eq!(sim.model().screen, Screen::Wallet);
    assert!(
        sim.model().send.to.value().is_empty(),
        "the form must clear"
    );
    // **S10: no id in the toast at all**, shortened or whole. It names where the payment is
    // instead, which is the only thing a reader can act on from a status bar.
    assert!(
        sim.model().note.contains("at the top of your payments"),
        "{}",
        sim.model().note
    );
    assert!(
        !sim.model().note.contains(BUILT_TXID)
            && !sim
                .model()
                .note
                .contains(&fmonero_core::short_id(BUILT_TXID)),
        "an id belongs in --json: {}",
        sim.model().note
    );
    // The relay announced the paid line through the sink, into the Log pane (§2a.2, §2g).
    let announced = sim.model().announce.lock().expect("sink").join("\n");
    let log_has_it = announced.contains("0.1000") || !sim.model().logs.empty;
    assert!(log_has_it, "the announce line is missing: {announced:?}");
}

/// §2i, in the TUI: `u`, `d` and `L` are refused under any layout but fmonero's own user
/// unit, **before** any RPC or confirm modal — and **each layout gets its own true sentence**.
///
/// **Round 5, S10.** One predicate (`may_mutate_node`, which is `layout == UserUnit`) answered both
/// `Layout::None` and `Layout::SystemUnit` with *"Another program on this machine runs this node.
/// fmonero can read it but not change it."* On a fresh machine in local mode — no units written,
/// nothing running, which is the first state this build's owner would have met — nothing ran that
/// node, the Node screen's hero invited `u` two rows above, and there was no in-app route to write
/// the units at all. This test is the pair: nobody set it up, and somebody else owns it.
#[test]
fn node_mutations_are_refused_with_the_sentence_that_is_true_of_this_machine() {
    // (1) `Layout::None`: no unit files, nothing running. The refusal names the route that fixes
    // it, and that route is a key.
    let mut m = offline_model(Screen::Node);
    for k in ['u', 'd', 'L'] {
        let _ = m.on_key(KeyEvent::new(KeyCode::Char(k)));
        assert!(
            m.modal.is_none(),
            "{k}: no confirm modal without a user unit"
        );
        assert!(m.action.is_none(), "{k}: nothing may be started");
        assert!(
            m.note.contains("has not set up your node yet") && m.note.contains("Press S"),
            "{k}: {}",
            m.note
        );
        assert!(
            !m.note.contains("Another program"),
            "{k}: nothing else runs this node: {}",
            m.note
        );
    }
    // (2) `Layout::SystemUnit`: Arch's own `monerod.service` is active, so fmonero reads it and
    // never changes it (§2i). Same three keys, the other sentence.
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-foreign-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    let fixture = dir.join("systemctl.fixture");
    std::fs::write(
        &fixture,
        "monerod.service active enabled yes 4242
",
    )
    .expect("fixture");
    ctx.systemctl = fmonero_core::Systemctl::Fixture(fixture);
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    let mut m = model(&ctx, Screen::Node);
    for k in ['u', 'd', 'L'] {
        let _ = m.on_key(KeyEvent::new(KeyCode::Char(k)));
        assert!(m.modal.is_none(), "{k}: no confirm modal");
        assert!(m.action.is_none(), "{k}: nothing may be started");
        assert!(
            m.note.contains("can read it but not change it"),
            "{k}: {}",
            m.note
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// §7a: an unresolved `pending.json` record is a red banner on **every** screen, carrying the
/// amount, what is known and where to look — and it turns the **wallet** badge red.
///
/// **v5.6 removed the two shell commands** (§2o.1): the banner named `wallet history --limit 5`
/// and `wallet clear-pending --yes` at a user who cannot type into it.
///
/// **Round 4 iteration 3 moved two things.** The badge: `node_state` returned `problem` for a
/// pending payment, so a synced node answering nineteen peers reported a fault in the three-cell
/// summary on every screen — the payment belongs to the wallet cell. And the counter: `0 of 1 sent`
/// is `relayed_count`, an acknowledgement fmonero never got, and printed as a count it asserts
/// that nothing went out, directly beside a Notes bullet saying it may already have gone. The
/// banner carries `PendingStatus::progress_words` instead, the same wording the widget and the
/// warning use.
/// **`wrap_bullet` dropped two characters at every break** (round 4 iteration 3), silently, with
/// no marker, whenever a bullet carried a token longer than the modal. It wrapped the whole string
/// to the full width and then prefixed two spaces to each continuation, so every continuation was
/// two columns too wide and the modal clipped it: `…workspaces-eff1` / `0e-d485-…`, with the `ae`
/// simply gone. The assertion is the one that matters — **every character survives, in order** —
/// plus the two shapes a hanging indent owes: no rendered line wider than the width, and every
/// continuation indented.
#[test]
fn a_wrapped_bullet_keeps_every_character_and_fits_its_width() {
    use crate::tui::screens::{wrap_bullet, wrap_hanging};
    use unicode_width::UnicodeWidthStr;
    let long =
        "/home/oliver/.config/Claude/scratch-workspaces-eff1ae0e-d485-4f42-9ffb-620b5efc18d5/x";
    for width in [20usize, 33, 46, 70, 74] {
        let bullet = format!("· nothing is lost either way. The blockchain stays in {long}");
        let lines = wrap_bullet(&bullet, width);
        for (i, l) in lines.iter().enumerate() {
            assert!(
                UnicodeWidthStr::width(l.as_str()) <= width,
                "line {i} is {} wide at width {width}: {l:?}",
                UnicodeWidthStr::width(l.as_str())
            );
            if i > 0 {
                assert!(
                    l.starts_with("  "),
                    "continuation {i} lost its indent: {l:?}"
                );
            }
        }
        // Every non-space character, in order, with nothing added and nothing lost.
        let want: String = bullet.chars().filter(|c| !c.is_whitespace()).collect();
        let got: String = lines
            .concat()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert_eq!(got, want, "characters lost or gained at width {width}");
    }
    // The three-space indent the open-a-wallet modal uses, on the same token.
    let lines = wrap_hanging(&format!("in {long}"), 30, 3);
    let want: String = format!("in {long}")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let got: String = lines
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(got, want);
    assert!(lines
        .iter()
        .all(|l| UnicodeWidthStr::width(l.as_str()) <= 30));
}

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
        assert_eq!(s.model().wallet_badge(), "payment may be out", "{screen:?}");
        assert_ne!(
            s.model().node_badge(),
            "problem",
            "a pending payment is not a fault of the node ({screen:?})"
        );
        for (cw, ch) in [(150u16, 40u16), (120, 40), (70, 24), (45, 20)] {
            let text = sim_text(&s, cw, ch);
            // The banner wraps at a narrow width, so it is checked on the joined text: what
            // matters is that nothing is CUT OFF, which is what a fixed two-row banner did at
            // 45 columns.
            let flat = text.replace('\n', " ");
            let flowed_frame = flowed(&text.replace('│', " "));
            // **S10: no id in the banner**, and no count of what was "sent": the three things
            // checked are the amount, what is actually known, and the route.
            // **Round 5, S8: the route is the one that works from where you are.** It read
            // `press 2 for the wallet` on the Wallet screen too, where `[ 2 Wallet ]` is already
            // the active tab — so on the one screen that can resolve the record it pointed at the
            // screen the reader was looking at. There it names the key that resolves it.
            let route = if screen == Screen::Wallet {
                "press P to clear this record once you know"
            } else {
                "press 2 for the wallet"
            };
            for needle in ["0.1000 XMR", "may already have gone out", route] {
                assert!(
                    flat.contains(needle),
                    "{screen:?} at {cw}x{ch} lost {needle}:\n{text}"
                );
            }
            // And the Wallet screen carries the chip the banner names (S8): a key named with no
            // control beside it is half of what §7a.2 asks for.
            if screen == Screen::Wallet {
                assert!(
                    flowed_frame.contains("[P] Clear that record"),
                    "the chip is missing at {cw}x{ch}:\n{text}"
                );
            }
            // §2o.1: no shell command anywhere in it.
            assert!(
                !flat.contains("fmonero wallet"),
                "{screen:?} at {cw}x{ch} printed a command:\n{text}"
            );
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
    assert!(text.contains("Public node"), "{text}");
    assert!(text.contains("node.example.com:18089"), "{text}");
    // **v5.6: the two panes that said "not available" are gone, and one pane of what IS known
    // stands in their place** (§7a.3, decision 3). The trust flag and the architecture sentence
    // are deleted outright (§2o).
    assert!(
        text.contains("This node"),
        "the one pane of what is known:\n{text}"
    );
    assert!(
        text.contains("Not checked yet."),
        "nothing has checked this address, and the pane says so:\n{text}"
    );
    assert!(text.contains("[p] Check this node"), "{text}");
    assert!(
        text.contains("Public nodes"),
        "the picker replaces the peers table in remote mode (§7a.3):\n{text}"
    );
    // §2o's own word list, over the rendered screen rather than the source — which is the half a
    // source grep cannot prove.
    for absent in [
        "not available",
        "untrusted",
        "trusted",
        "ssl",
        "monero-wallet-rpc",
        "daemon",
        "probe",
        "eta ",
        "Node is not running",
        "sparkline fills",
    ] {
        assert!(
            !text.contains(absent),
            "the public-node screen rendered `{absent}`:\n{text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ======================================================================================
// v5.5 — §7a.1 navigation, §7a.2 mouse, §7a.4 the picker, §7a.5 the wallet and daemon flow
// ======================================================================================

use super::hit::{Button, HitPane, Target};
use ftui::{MouseButton, MouseEvent, MouseEventKind};

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> Event {
    Event::Mouse(MouseEvent::new(kind, x, y))
}

fn click(x: u16, y: u16) -> Event {
    mouse(MouseEventKind::Down(MouseButton::Left), x, y)
}

/// **The one non-obvious precondition of every mouse test** (§7a.2): the layout cache is empty
/// until a frame has been rendered, so a test that injects a click before its first render is
/// testing `Map::at` on zeroed rects. This renders, then finds the coordinate of the first cell
/// whose target satisfies `want` — so the test never hard-codes a column that a layout change
/// would silently move.
fn find(m: &Tui, w: u16, h: u16, want: impl Fn(Target) -> bool) -> Option<(u16, u16)> {
    let _ = render(m, w, h);
    (0..h).find_map(|y| {
        (0..w).find_map(|x| match m.hits.at(x, y, None) {
            Some(t) if want(t) => Some((x, y)),
            _ => None,
        })
    })
}

/// §7a.1's matrix, and it is a matrix because the defect was reachable three ways: `Tab` was
/// dispatched per screen while the filter branch and `send.rs`'s `Tab | Down` each ate it first.
#[test]
fn tab_switches_screens_from_every_screen_and_every_focus_state() {
    /// One cell of the matrix: what to do before pressing `Tab`.
    type Arrange = fn(&mut ProgramSimulator<Tui>);
    let cases: Vec<(&str, Arrange)> = vec![
        ("nothing focused", |_s| {}),
        ("the Wallet filter open with text in it", |s| {
            s.inject_event(key(KeyCode::Char('2')));
            s.inject_event(key(KeyCode::Char('/')));
            type_str(s, "abc");
        }),
        ("the Log search open", |s| {
            s.inject_event(key(KeyCode::Char('4')));
            s.inject_event(key(KeyCode::Char('/')));
            type_str(s, "sync");
        }),
        ("a Send field focused with an address typed", |s| {
            s.inject_event(key(KeyCode::Char('3')));
            type_str(s, DEST);
        }),
        ("the picker's filter open", |s| {
            s.inject_event(key(KeyCode::Char('1')));
            s.inject_event(key(KeyCode::Char('/')));
            type_str(s, "example");
        }),
        ("a confirm modal open", |s| {
            s.inject_event(key(KeyCode::Char('1')));
            s.inject_event(key(KeyCode::Char('u')));
        }),
        ("the ? overlay up", |s| {
            s.inject_event(key(KeyCode::Char('?')));
        }),
        ("an add-node modal open", |s| {
            s.inject_event(key(KeyCode::Char('1')));
            s.inject_event(key(KeyCode::Char('a')));
        }),
    ];
    for (what, arrange) in cases {
        for from in [Screen::Node, Screen::Wallet, Screen::Send, Screen::Log] {
            let mut s = sim(offline_model(from));
            arrange(&mut s);
            let before = s.model().screen;
            s.inject_event(key(KeyCode::Tab));
            assert_eq!(
                s.model().screen,
                Screen::from_index(before.index() + 1),
                "Tab must advance one screen from {before:?} with {what}"
            );
        }
    }
}

#[test]
fn shift_tab_goes_back_from_every_screen() {
    for from in [Screen::Node, Screen::Wallet, Screen::Send, Screen::Log] {
        let mut s = sim(offline_model(from));
        s.inject_event(key(KeyCode::BackTab));
        assert_eq!(s.model().screen, Screen::from_index(from.index() + 3));
    }
}

/// The two documented exceptions, and **only** two: a built transaction and a typed password are
/// the two states where "go somewhere else" must be an explicit discard (§7a.1).
#[test]
fn the_create_wallet_modal_consumes_tab_and_says_so() {
    let mut s = sim(offline_model(Screen::Wallet));
    s.inject_event(key(KeyCode::Char('c')));
    type_str(&mut s, "secret-ish");
    let before = s.model().screen;
    s.inject_event(key(KeyCode::Tab));
    assert_eq!(s.model().screen, before, "the create modal holds Tab");
    assert_eq!(s.model().note, "Press Esc first to discard this.");
    // And it is documented IN the modal — on its own line under the controls, which is a
    // rendering assertion and not a status-bar one: a one-row footer clipped that line away, so
    // the rule was true only in DESIGN.md.
    let text = sim_text(&s, 110, 34);
    // M23: `first` no longer dangles — what Tab normally does is change screens.
    assert!(
        flowed(&text).contains("Tab cannot change screens while this is open."),
        "{text}"
    );
}

#[test]
fn the_send_confirm_consumes_tab() {
    let w = World::new("tab-confirm");
    let mut s = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    s.init();
    type_str(&mut s, DEST);
    s.inject_event(key(KeyCode::Down));
    type_str(&mut s, "0.1");
    s.inject_event(key(KeyCode::Enter));
    run_action(&mut s);
    assert!(matches!(s.model().send.stage, Stage::Built(_)));
    s.inject_event(key(KeyCode::Tab));
    assert_eq!(s.model().screen, Screen::Send, "a built plan holds Tab");
    assert!(
        matches!(s.model().send.stage, Stage::Built(_)),
        "not dropped"
    );
}

/// A focused filter is **committed** by `Tab`, not discarded: coming back with `Shift+Tab` must
/// find the filter the user built, which is what makes leaving safe enough to be the default.
#[test]
fn tab_out_of_a_filter_keeps_what_was_typed() {
    let mut s = sim(offline_model(Screen::Wallet));
    s.inject_event(key(KeyCode::Char('/')));
    type_str(&mut s, "abc");
    s.inject_event(key(KeyCode::Tab));
    assert_eq!(s.model().screen, Screen::Send);
    assert!(!s.model().wallet.filter_open, "the field lost focus");
    assert_eq!(
        s.model().wallet.filter.value(),
        "abc",
        "Tab commits the filter — Esc is what discards"
    );
    s.inject_event(key(KeyCode::BackTab));
    assert_eq!(s.model().screen, Screen::Wallet);
    assert_eq!(s.model().wallet.filter.value(), "abc");
}

#[test]
fn a_click_on_each_tab_switches_to_it() {
    for (i, want) in [
        (0, Screen::Node),
        (1, Screen::Wallet),
        (2, Screen::Send),
        (3, Screen::Log),
    ] {
        for (w, h) in [(120u16, 40u16), (56, 24)] {
            let mut s = sim(offline_model(Screen::Log));
            let at = find(s.model(), w, h, |t| t == Target::Tab(i))
                .unwrap_or_else(|| panic!("tab {i} must be a hit target at {w}x{h}"));
            s.inject_event(click(at.0, at.1));
            assert_eq!(s.model().screen, want, "a click on tab {i} at {w}x{h}");
        }
    }
}

/// `TableState`'s own rule, reused: the second click on the already-selected row is an
/// activation, so "click to select, click again to open" replaces double-click detection
/// entirely and there is no interval to get wrong on a slow terminal.
#[test]
fn a_click_selects_a_row_and_a_second_click_activates_it() {
    let w = World::new("click-row");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    let at = find(s.model(), 120, 40, |t| {
        matches!(
            t,
            Target::Row {
                pane: HitPane::History,
                ..
            }
        )
    });
    let Some((x, y)) = at else {
        return; // no history rows in this fixture world: the picker case below covers the rule
    };
    // Clear whatever the screen had selected: the rule under test is "a click on a row that is
    // NOT selected selects it; a click on the one that IS selected activates it", and starting
    // from a pre-selected row 0 would test only the second half.
    s.model().wallet.list.borrow_mut().selected = None;
    s.inject_event(click(x, y));
    let selected = s.model().wallet.list.borrow().selected;
    assert!(selected.is_some(), "the first click selects");
    assert!(!s.model().wallet.detail, "and does NOT open");
    let _ = render(s.model(), 120, 40);
    s.inject_event(click(x, y));
    assert!(
        s.model().wallet.detail || s.model().selected_transfer().is_none(),
        "the second click on the same row activates it"
    );
}

#[test]
fn a_wheel_scrolls_the_list_under_the_cursor_and_never_the_tab_strip() {
    let w = World::new("wheel");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    let over_tabs = find(s.model(), 120, 40, |t| t == Target::Tab(0)).expect("a tab");
    let before = s.model().screen;
    s.inject_event(mouse(MouseEventKind::ScrollDown, over_tabs.0, over_tabs.1));
    assert_eq!(
        s.model().screen,
        before,
        "a wheel over the tab strip must do nothing: it would change screens while someone \
         scrolls a list near the top edge"
    );
}

#[test]
fn a_click_on_the_overlay_closes_it_and_does_not_switch_screens() {
    let mut s = sim(offline_model(Screen::Node));
    s.inject_event(key(KeyCode::Char('?')));
    assert!(s.model().show_help);
    // The overlay is priority 1: a click on a TAB closes the overlay and changes nothing else.
    let at = find(s.model(), 120, 40, |t| t == Target::Button(Button::Help)).expect("a hit");
    s.inject_event(click(at.0, at.1));
    assert!(!s.model().show_help, "the overlay closed");
    assert_eq!(
        s.model().screen,
        Screen::Node,
        "and the screen did not move"
    );
}

/// §7a.2: "cancellation and money are untouched by the mouse". A misplaced click must never
/// discard a built transaction.
#[test]
fn a_backdrop_click_does_not_discard_a_built_transaction() {
    let w = World::new("backdrop");
    let mut s = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    s.init();
    type_str(&mut s, DEST);
    s.inject_event(key(KeyCode::Down));
    type_str(&mut s, "0.1");
    s.inject_event(key(KeyCode::Enter));
    run_action(&mut s);
    assert!(matches!(s.model().send.stage, Stage::Built(_)));
    let _ = render(s.model(), 110, 34);
    // The top-left cell is outside every modal box this program draws.
    s.inject_event(click(0, 2));
    assert!(
        matches!(s.model().send.stage, Stage::Built(_)),
        "the plan survives a stray click: {}",
        s.model().note
    );
}

/// Revision 6: every control the keymap reaches with `←→` renders its two arrows as targets.
/// Without this a mouse-only user could not change the fee priority or the sending account.
#[test]
fn the_priority_chips_and_the_account_arrows_are_clickable() {
    let w = World::new("send-chips");
    let mut s = sim(model(&w.ctx, Screen::Send));
    let at = find(s.model(), 120, 40, |t| t == Target::Priority(4)).expect("a priority chip");
    s.inject_event(click(at.0, at.1));
    assert_eq!(
        s.model().send.priority,
        4,
        "a click SELECTS that priority rather than cycling"
    );
    let arrow = find(s.model(), 120, 40, |t| {
        matches!(
            t,
            Target::Arrow {
                which: super::hit::Selector::Account,
                delta: 1
            }
        )
    });
    assert!(arrow.is_some(), "the account selector's ▸ must be a target");
}

/// §7a.2's rule that if a screen tells the user to press a key, that text is the button — and it
/// does what the key does, through **one** code path.
#[test]
fn every_wallet_empty_state_chip_is_a_hit_target_and_acts() {
    let mut s = sim(offline_model(Screen::Wallet));
    // One wallet on this machine, so `[o]` has something to open: the chip is drawn only where it
    // can act (round 4 iteration 3), and this test is about the chips that are drawn.
    s.model_mut().wallets = vec!["main".to_string()];
    for (b, check) in [
        (Button::OpenWallet, "Open a wallet"),
        (Button::CreateWallet, "Create a wallet"),
    ] {
        let at = find(s.model(), 120, 40, |t| t == Target::Button(b))
            .unwrap_or_else(|| panic!("{b:?} must be a hit target"));
        s.inject_event(click(at.0, at.1));
        let text = sim_text(&s, 120, 40);
        assert!(
            text.contains(check),
            "clicking {b:?} must open its modal:\n{text}"
        );
        s.inject_event(key(KeyCode::Escape));
    }
    // `[W]` is a hit target in the one state it can act in: the service up with nothing open.
    if let Some(st) = s.model_mut().status.as_mut() {
        st.wallet.unit_active = true;
        st.wallet.open = false;
        st.wallet.name = None;
    }
    assert!(
        find(s.model(), 120, 40, |t| t
            == Target::Button(Button::WalletDaemon))
        .is_some(),
        "[W] Stop the wallet service must be clickable in the state it can act in"
    );
}

/// §7a.5's empty state, at the **Narrow** tier too: a screen whose whole content is three
/// buttons has nothing to drop, and the tier where a user is most likely to be stuck is not the
/// tier to hide the way out.
/// **One state, one word** (round 4 iteration 3). This badge said `closed` where §5 raises
/// `wallet_idle` and `not running` where it does not — the same fact, the service down with
/// nothing open, read two ways depending on which mode the reader was in, while the Wallet
/// screen's own row in the same frame said a third thing. The row's wording wins: `not running`.
/// Calm-versus-alarm is the warning's job and the bar icon's; both branches painted the same tone.
#[test]
fn the_status_bar_says_one_thing_when_the_wallet_service_is_down() {
    let w = World::new("idle-badge");
    let mut ctx = w.ctx.clone();
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = "node.example.com:18089".into();
    // A port nothing listens on: the wallet daemon is absent, which in remote mode with no wallet
    // recorded open is `wallet_idle` (§5), not `wallet_down`.
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    let s = sim(model(&ctx, Screen::Wallet));
    assert!(
        s.model().has_warning("wallet_idle"),
        "the fixture must actually reach the wallet_idle state, or this proves nothing"
    );
    assert_eq!(s.model().wallet_badge(), "not running");
    let text = sim_text(&s, 120, 40);
    assert!(text.contains("wallet not running"), "{text}");
    assert!(
        !text.contains("wallet closed"),
        "the frame must not offer a second word for this state:\n{text}"
    );
}

/// **V3, as a render test.** Decision 7's own two lines — the ones v5.6 added so that a user who
/// closes the window knows the scanning goes on — were drawn on a bare rect in the gap between two
/// framed panes, at column 0, which made them the only flush-left prose in the program. The Node
/// screen puts the same pair inside its hero pane; the Wallet screen now puts them inside
/// ` Balance `, which is why that pane is five rows tall while a wallet is catching up.
///
/// The assertion is structural rather than a string match: on this screen, every row between the
/// tab strip and the status line either belongs to a frame or is a chip row. A chip row is the one
/// deliberate exception — `[R] Check for payments` is a button strip, not prose — so it is named
/// here rather than silently allowed by a weaker check.
#[test]
fn nothing_on_the_wallet_screen_is_drawn_outside_a_frame() {
    let w = World::with_wallet("outside-frame", &["--scenario", "behind"]);
    let s = sim(model(&w.ctx, Screen::Wallet));
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let lines = render(s.model(), cols, rows);
        // The fixture pair is 6 122 blocks apart, so decision 7's lines really are on screen —
        // without this the test would pass on a screen that never rendered them.
        let text = lines.join("\n");
        assert!(
            text.contains(fmonero_core::SYNC_KEEPS_GOING),
            "{cols}x{rows}: decision 7's lines are not on screen at all:\n{text}"
        );
        for (y, line) in lines.iter().enumerate() {
            // Row 0 is the tab strip and the last row is the status bar: chrome, by design.
            if y == 0 || y + 1 >= lines.len() {
                continue;
            }
            let t = line.trim_start();
            let Some(first) = t.chars().next() else {
                continue;
            };
            assert!(
                "╭╮╰╯│─├┤┌┐└┘".contains(first) || t.starts_with('['),
                "{cols}x{rows}: row {y} is drawn outside every frame: {line:?}\n{text}"
            );
        }
    }
}

#[test]
fn the_wallet_empty_state_keeps_every_chip_it_draws_at_every_tier() {
    // One wallet, so `[o]` is drawn; `[W]` is not, because opening a wallet starts the service and
    // the chip that said so in its own parenthetical is gone (round 4 iteration 3).
    let mut s = sim(offline_model(Screen::Wallet));
    s.model_mut().wallets = vec!["main".to_string()];
    for (w, h) in [(120u16, 40u16), (70, 24), (56, 22)] {
        let text = sim_text(&s, w, h);
        for want in ["[o] Open a wallet", "[c] Create a wallet"] {
            assert!(text.contains(want), "{want} missing at {w}x{h}:\n{text}");
        }
        assert!(
            !text.contains("[W] Start the wallet service"),
            "a chip whose own label says not to press it is not drawn at {w}x{h}:\n{text}"
        );
    }
}

// ---- §7a.4 the picker ----------------------------------------------------------------

/// Write a cache the picker can render, with no network and no `curl` (§2n.6).
fn seed_cache(ctx: &Context, rows: &[(&str, bool)]) {
    let nodes: Vec<serde_json::Value> = rows
        .iter()
        .map(|(addr, probed)| {
            serde_json::json!({
                "url": format!("http://{addr}"), "address": addr, "scheme": "http",
                "web_compatible": false, "custom": false,
                "health_available": true, "health_ratio": 0.5, "health_checks": 18,
                "health_last_height": 3_765_000u64,
                "health_checked_at_text": "Fri, 18 Sep 2026 05:12:02 GMT",
                "health_checked_at": fmonero_core::now_unix() - 600,
                "probe": probed.then(|| serde_json::json!({
                    "reachable": true, "height": 3_765_100u64, "synchronized": true,
                    "restricted": true, "nettype": "mainnet", "version": null,
                    "latency_ms": 42, "reason": null, "probed_at": fmonero_core::now_unix()
                })),
            })
        })
        .collect();
    std::fs::create_dir_all(&ctx.paths.cache_dir).expect("cache dir");
    std::fs::write(
        ctx.paths.public_nodes(),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1, "fetched_at": fmonero_core::now_unix(), "source": "https://monero.fail",
            "skipped_no_port": 37, "skipped_invalid": 0, "notice_ack": fmonero_core::now_unix(),
            "nodes": nodes
        }))
        .expect("json"),
    )
    .expect("write the cache");
}

#[test]
fn the_picker_renders_the_cache_on_entry_and_shows_both_column_groups() {
    let w = World::new("picker-render");
    seed_cache(
        &w.ctx,
        &[("a.example:18081", true), ("b.example:18081", false)],
    );
    let s = sim(model(&w.ctx, Screen::Node));
    let text = sim_text(&s, 120, 44);
    assert!(text.contains("Public nodes"), "{text}");
    assert!(text.contains("a.example:18081"), "{text}");
    // The ratio WITH its sample size: 100 % of 3 and 72 % of 94 are not comparable (§2n.1).
    assert!(text.contains("50% of 18"), "{text}");
    // The probed row carries this machine's own measurement; the unprobed one carries `—`.
    assert!(
        text.contains("42"),
        "the latency of the probed row:\n{text}"
    );
    // W6: the footer says what was skipped and what it counts, and pluralises both. The cache
    // this test seeds has 37 portless rows and no unreadable ones, so the second clause is absent
    // — a count of zero draws no clause.
    // The Notes pane is drawn on the same rows to the right of this pane, so the footer's own
    // wrap is asserted in two halves rather than by flowing the whole frame.
    let flat = flowed(&text);
    assert!(
        flat.contains("37 without a port") && flat.contains("left out"),
        "the footer must say what was left out:\n{text}"
    );
    assert!(
        flat.contains("2 nodes from monero.fail"),
        "and pluralise its own count:\n{text}"
    );
    // And it is wrapped rather than cut: nothing in the footer's own rows ends in an ellipsis.
    for line in text.lines().filter(|l| l.contains("monero.fail")) {
        assert!(
            !line.contains('…'),
            "the footer was cut instead of wrapped: {line}"
        );
    }
}

/// **V6 and V7, on the strings the verification read off a real pty.** The footer said
/// `list 3s ago old`, the reliability cell was cut to `72% of 18 ch` by its own 12-cell column, and
/// two columns at the Wide tier were both headed `block` — monero.fail's height and the height this
/// machine measured, with nothing to say which was which.
#[test]
fn the_pickers_footer_reads_as_english_and_no_two_columns_share_a_header() {
    let w = World::new("picker-words");
    // Public-node mode, which is where the picker is the screen's main pane and wide enough for
    // both column groups — and the mode the verification read this table in.
    let mut ctx = w.ctx.clone();
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = "a.example:18081".into();
    seed_cache(
        &ctx,
        &[("a.example:18081", true), ("b.example:18081", false)],
    );
    let s = sim(model(&ctx, Screen::Node));
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cols, rows);
        assert!(
            text.contains("list fetched "),
            "{cols}x{rows}: the footer is not a sentence:\n{text}"
        );
        assert!(
            !text.contains(" old ·"),
            "{cols}x{rows}: `ago old` survives:\n{text}"
        );
        // The cell fits its column, so the sample size travels with the percentage (§2n.1).
        assert!(text.contains("50% of 18"), "{cols}x{rows}:\n{text}");
        assert!(
            !text.contains("of 18 ch"),
            "{cols}x{rows}: the reliability cell is still clipped:\n{text}"
        );
        // Two headers, two names, and neither is the bare `block` both used to be. The header row
        // is whichever columns fit this tier, so the assertion is on the shape rather than on the
        // set: no two cells in it may read the same, and monero.fail's height is always named.
        let header = text
            .lines()
            .find(|l| l.contains("address") && l.contains("answered"))
            .unwrap_or_default();
        assert!(header.contains("their block"), "{cols}x{rows}:\n{text}");
        let names: Vec<&str> = header
            .split('│')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect();
        let mut seen = names.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            names.len(),
            "{cols}x{rows}: two columns share a header: {names:?}"
        );
    }
    // Wide enough for both groups, which is where the defect was visible: `block` twice, with
    // nothing to say which was monero.fail's and which was ours. The picker shares its row with
    // the Notes pane, so both groups need more than the 150 columns the verification ran at —
    // which is itself worth pinning, because the pair is what the column names exist to separate.
    let wide = sim_text(&s, 210, 40);
    assert!(wide.contains("their block"), "{wide}");
    assert!(wide.contains("block here"), "{wide}");
}

/// **V4.** `l` — the in-app route back to your own node, which v5.6 added precisely because §2o.1
/// forbids the TUI from printing `fmonero node local --yes` — opened a confirm that stated nothing:
/// `y confirms · Esc cancels`, plus the wallet's name when one was open. §7a.4 specifies three
/// sentences, and the picker's own `Enter` confirm two keys away already had that shape.
#[test]
fn the_use_your_own_node_confirm_states_what_the_switch_does() {
    let w = World::new("confirm-local");
    let mut ctx = w.ctx.clone();
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = "a.example:18081".into();
    seed_cache(&ctx, &[("a.example:18081", false)]);
    let mut s = sim(model(&ctx, Screen::Node));
    s.inject_event(key(KeyCode::Char('l')));
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cols, rows);
        assert!(text.contains("Use your own node"), "{cols}x{rows}:\n{text}");
        for want in [
            // M10: the service is not running in this fixture, so the modal must not promise a
            // restart of it.
            "will use your own node when it next starts",
            "Your own node is not running yet",
            "y to confirm",
        ] {
            assert!(
                text.contains(want),
                "{cols}x{rows}: the confirm does not say {want:?}:\n{text}"
            );
        }
        // §7a.4: never a command, in the modal the whole feature exists to replace one with.
        assert!(
            !text.contains("fmonero node local"),
            "{cols}x{rows}: the modal prints the command it replaced:\n{text}"
        );
    }
    // And it claims nothing it does not know: this sandbox has no record of a wallet by name
    // (`state.json` is empty), so the modal says nothing about closing one. §7a.4 names the wallet
    // only when there is a name to give.
    let named = s
        .model()
        .status
        .as_ref()
        .and_then(|st| st.wallet.name.clone())
        .is_some();
    let text = sim_text(&s, 150, 40);
    assert_eq!(
        text.contains("will be closed"),
        named,
        "the wallet clause must appear exactly when there is a wallet to name:\n{text}"
    );
}

/// **V15, and W14 on top of it.** Nothing a user reads is cut mid-word.
///
/// V15's finding was a toast cut wherever the status bar's row happened to end —
/// `A wallet is open. Press x to close i`, a complete sentence about a key that does not exist —
/// and its fix was an ellipsis at a word boundary. W14 found the cost of that fix: the refusal the
/// whole of §2o.1 is built on is 40 columns and the bar had 18 of them, so an honest mark was all
/// the user ever saw of it. A toast that does not fit beside the badges now has a row of its own,
/// wrapped, and this test reads it at the two sizes the captures are read at.
#[test]
fn a_cut_line_always_says_it_was_cut() {
    let mut s = sim(offline_model(Screen::Node));
    let long = "A wallet is open. Press x to close it, and then W stops the wallet service for good measure.";
    s.model_mut().note_for_test(long);
    for (cols, rows) in [(120u16, 32u16), (80, 24)] {
        let text = sim_text(&s, cols, rows);
        assert!(
            flowed(&text).contains(long),
            "{cols}x{rows}: the toast must be on screen whole:\n{text}"
        );
        // And it is never cut mid-word: no row that carries part of it ends in an ellipsis.
        for line in text.lines().filter(|l| l.contains("A wallet is open")) {
            assert!(
                !line.contains('…'),
                "{cols}x{rows}: the toast was cut: {line}"
            );
        }
    }
    // A short toast still shares the bar with the badges, which is the whole point of the bar.
    s.model_mut().note_for_test("Copied.");
    let text = sim_text(&s, 120, 32);
    let bar = text.lines().last().unwrap_or_default();
    assert!(
        bar.contains("Copied."),
        "a short toast stays on the bar:\n{text}"
    );

    // **W13: the `?` overlay wraps, and nothing in it ends in `…`.** V15 marked its cut rows,
    // which made them honest and still unreadable — 11 of 25 at 120 columns and 9 at 150,
    // including the rows that say why `u`, `d` and `L` are unavailable. Read at both sizes, in
    // both modes, and every row of `KEYS` has to be present **whole**.
    s.inject_event(key(KeyCode::Char('?')));
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cols, rows);
        let top = text
            .lines()
            .position(|l| l.contains("Keys · any key closes"))
            .unwrap_or_else(|| panic!("the overlay is open at {cols}x{rows}:\n{text}"));
        // The overlay's own rows and its own COLUMNS. The screen behind it is still drawn to the
        // left and right of them, and the status bar below it carries a toast that IS elided on
        // purpose (V15, above) — either one lands inside a wrapped sentence otherwise.
        let title_row = text.lines().nth(top).unwrap_or_default();
        let x0 = title_row.chars().position(|c| c == '╭').unwrap_or(0);
        let x1 = title_row
            .chars()
            .position(|c| c == '╮')
            .unwrap_or_else(|| title_row.chars().count());
        // From the overlay's own left border to its own right one, so the row that ends the
        // overlay is recognisable by its first character.
        let column =
            |l: &str| -> String { l.chars().skip(x0).take(x1.saturating_sub(x0) + 1).collect() };
        let body: Vec<String> = text
            .lines()
            .skip(top + 1)
            .map(column)
            .take_while(|l| !l.starts_with('╰'))
            .collect();
        assert!(
            body.len() > KEYS.len(),
            "{cols}x{rows}: the overlay drew {} rows for {} keys:\n{text}",
            body.len(),
            KEYS.len()
        );
        for line in &body {
            assert!(
                !line.contains('…'),
                "{cols}x{rows}: a help row was elided: {line}\n{text}"
            );
        }
        // Every action, whole — wrapped rows are joined before the comparison, which is the only
        // way to assert a sentence that spans two of them. The overlay's own border columns (and
        // the screen's behind it) come out first, or they would land inside the sentence.
        let flat = flowed(&body.join(" ").replace('│', " "));
        for (k, action) in KEYS {
            assert!(
                flat.contains(&flowed(action)),
                "{cols}x{rows}: the {k} row is not on screen whole:\n{text}"
            );
        }
    }
}

/// **V11.** The modal showed neither what was wrong with what the user had typed nor which key
/// commits it: `Enter adds it · Esc cancels` was the third line passed to a two-row interior, so no
/// capture at any size has ever contained it. §7a.4 also asks for live validation through
/// `Settings::parse_remote_address` — the same function the CLI refuses with — and the call did
/// nothing of the kind.
#[test]
fn the_add_node_modal_validates_as_you_type_and_shows_the_key_that_commits() {
    let w = World::new("add-node-modal");
    seed_cache(&w.ctx, &[("a.example:18081", false)]);
    let mut s = sim(model(&w.ctx, Screen::Node));
    s.inject_event(key(KeyCode::Char('a')));
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cols, rows);
        assert!(text.contains("Add a node"), "{cols}x{rows}:\n{text}");
        assert!(
            text.contains("Enter adds it"),
            "{cols}x{rows}: the key that commits is still off the bottom:\n{text}"
        );
        assert!(
            text.contains("host:port, like node.example.com:18089"),
            "{cols}x{rows}: an empty field gets the example:\n{text}"
        );
    }
    // Live validation: §2m.2's own message for whatever is wrong, as the user types.
    type_str(&mut s, "nonsense");
    let text = sim_text(&s, 120, 32);
    assert!(
        text.contains("The port is required"),
        "the refusal is not under the field:\n{text}"
    );
    // And a valid address says so, with the key that adds it.
    for _ in 0..8 {
        s.inject_event(key(KeyCode::Backspace));
    }
    type_str(&mut s, "node.example.com:18089");
    let text = sim_text(&s, 120, 32);
    assert!(
        text.contains("node.example.com:18089 · press Enter to add it"),
        "a valid address is not confirmed:\n{text}"
    );
}

/// Revision 4, both guards: nothing is selected on entry, and the configured row refuses
/// activation. Either alone still leaves a shape where one click costs the user something.
#[test]
fn the_picker_selects_nothing_on_entry_and_refuses_the_configured_node() {
    let w = World::new("picker-guards");
    let mut ctx = w.ctx.clone();
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = "mine.example:18089".into();
    seed_cache(
        &ctx,
        &[
            ("mine.example:18089", false),
            ("other.example:18081", false),
        ],
    );
    let mut s = sim(model(&ctx, Screen::Node));
    assert_eq!(
        s.model().picker.table.borrow().selected,
        None,
        "no row is selected on entry, so a first click can only select"
    );
    // `Enter` with nothing selected does not open a confirm either.
    s.inject_event(key(KeyCode::Enter));
    assert!(s.model().modal.is_none(), "{}", s.model().note);
    // The configured node sorts first and refuses activation with a toast.
    s.inject_event(key(KeyCode::Down));
    assert_eq!(s.model().picker.table.borrow().selected, Some(0));
    s.inject_event(key(KeyCode::Enter));
    assert!(
        s.model().modal.is_none(),
        "activating the node already in use must not open the confirm"
    );
    assert_eq!(s.model().note, "This is the node you are using.");
    // The next row DOES open it, with all three consequences named.
    s.inject_event(key(KeyCode::Down));
    s.inject_event(key(KeyCode::Enter));
    let text = sim_text(&s, 120, 44);
    assert!(text.contains("other.example:18081"), "{text}");
    // §2o deletes the flag and states the fact behind it instead.
    assert!(!text.contains("untrusted"), "{text}");
    assert!(
        text.contains("will not tell this node which amounts"),
        "{text}"
    );
    // M10: the service restarts only when it is running, and this fixture's is not.
    assert!(
        flowed(&text).contains("will use this node when it next starts"),
        "{text}"
    );
    assert!(text.contains("IP address"), "{text}");
    // M12: what the last check of this node found, or that nothing has checked it.
    assert!(
        text.contains("checked") || text.contains("nothing has checked"),
        "{text}"
    );
}

/// Revision 8: the no-cache state is a panel with a focused `[f]` chip and the disclosure — not
/// an empty table under a footer rendered against a `fetched_at` that does not exist.
#[test]
fn the_picker_with_no_cache_offers_the_fetch_chip_and_prints_no_broken_footer() {
    let w = World::new("picker-empty");
    let s = sim(model(&w.ctx, Screen::Node));
    let text = sim_text(&s, 120, 44);
    assert!(text.contains("No list of public nodes yet."), "{text}");
    assert!(text.contains("[f] Get the list"), "{text}");
    assert!(text.contains("tells monero.fail your IP address"), "{text}");
    assert!(
        !text.contains("list  old") && !text.contains("0 nodes ·"),
        "the footer must be suppressed, not rendered with empty fields:\n{text}"
    );
    assert!(
        find(s.model(), 120, 44, |t| t
            == Target::Button(Button::RefreshList))
        .is_some(),
        "the chip is a hit target too"
    );
}

/// §2n.5 rule 2 with revision 7's record: the notice comes up before the first fetch **or** the
/// first probe while `notice_ack` is unset, and nothing is contacted until it is answered.
#[test]
fn the_privacy_notice_stands_in_front_of_the_first_fetch_and_the_first_probe() {
    for (keypress, what) in [(KeyCode::Char('f'), "fetch"), (KeyCode::Char('p'), "probe")] {
        let w = World::new(&format!("notice-{what}"));
        let mut s = sim(model(&w.ctx, Screen::Node));
        s.inject_event(key(keypress));
        let text = sim_text(&s, 120, 44);
        assert!(
            text.contains("Getting the list tells monero.fail your IP address"),
            "the notice must precede the first {what}:\n{text}"
        );
        assert!(s.model().action.is_none(), "and nothing may be armed yet");
        // Esc contacts nobody and writes no record.
        s.inject_event(key(KeyCode::Escape));
        assert!(!fmonero_core::nodes::notice_acknowledged(&w.ctx));
    }
}

// ---- §7a.5 create --------------------------------------------------------------------

/// Revision 1, and it is the test the blocker asked for: **all four controls filled from the
/// keyboard alone.** Without a field key this modal is a trap, because it is one of the two that
/// consume `Tab`.
#[test]
fn the_create_modal_is_fillable_from_the_keyboard_alone() {
    let w = World::new("create-keyboard");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    s.inject_event(key(KeyCode::Char('c')));
    type_str(&mut s, "fresh");
    s.inject_event(key(KeyCode::Down));
    type_str(&mut s, "hunter2");
    s.inject_event(key(KeyCode::Down));
    type_str(&mut s, "hunter2");
    s.inject_event(key(KeyCode::Down));
    // The language selector, reached and changed with `←→`.
    s.inject_event(key(KeyCode::Right));
    let text = sim_text(&s, 110, 34);
    assert!(text.contains("fresh"), "the name is still there:\n{text}");
    assert!(
        !text.contains("hunter2"),
        "both password fields are masked (§2b.4):\n{text}"
    );
    s.inject_event(key(KeyCode::Enter));
    assert!(
        s.model().action.is_some(),
        "Enter on the last control creates: {}",
        s.model().note
    );
    let armed = format!("{:?}", s.model().action.as_ref().map(|a| a.label.clone()));
    assert!(armed.contains("Creating fresh"), "{armed}");
}

/// Every local refusal §7a.5 names, and **nothing is sent** for any of them.
#[test]
fn the_create_modal_refuses_a_slash_a_mismatch_and_an_empty_password() {
    let w = World::new("create-refusals");
    for (name, p1, p2, want) in [
        ("a/b", "x", "x", "cannot contain"),
        ("ok", "one", "two", "do not match"),
        ("ok", "", "", "unencrypted"),
        ("main", "x", "x", "already exists"),
    ] {
        let mut s = sim(model(&w.ctx, Screen::Wallet));
        s.inject_event(key(KeyCode::Char('c')));
        type_str(&mut s, name);
        s.inject_event(key(KeyCode::Down));
        type_str(&mut s, p1);
        s.inject_event(key(KeyCode::Down));
        type_str(&mut s, p2);
        s.inject_event(key(KeyCode::Down));
        s.inject_event(key(KeyCode::Enter));
        assert!(
            s.model().action.is_none(),
            "'{name}' must not start a create"
        );
        assert_eq!(w.count("create_wallet"), 0, "nothing may be sent");
        let text = sim_text(&s, 110, 34);
        assert!(text.contains(want), "expected '{want}' in:\n{text}");
    }
}

/// §7a.5, and the assertion is by **word count against the rendered buffer**, because "we never
/// print the seed" has to be a test and not a habit.
#[test]
fn the_seed_backup_modal_names_the_command_and_contains_no_seed() {
    let w = World::new("seed-modal");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    s.send(Msg::ActionDone(Box::new(Ok(subs::Outcome::Created {
        name: "fresh".into(),
        hint: fmonero_core::wallet_session::seed_backup_hint(&w.ctx, "fresh"),
    }))));
    let text = sim_text(&s, 110, 34);
    assert!(text.contains("monero-wallet-cli"), "{text}");
    assert!(text.contains("--offline"), "{text}");
    // Revision 11: the close step, because `create_wallet` leaves the wallet OPEN in the wallet
    // service. **v5.6 states it without naming a command** (§2o.1): the modal says the wallet
    // must be closed, and `x` is the key that does it.
    assert!(text.contains("With the wallet closed"), "{text}");
    assert!(!text.contains("fmonero wallet close"), "{text}");
    assert!(text.contains("never shows your seed"), "{text}");
    // No line of the modal is 25 words long, which is what a seed would be. The **status bar**
    // is excluded by row, not by threshold: it is a strip of short labelled fields — two badges,
    // a grouped block number (W7), two peer counts, a balance and a toast — and raising the
    // word limit until that row passes is raising it past a seed.
    let rows = render(s.model(), 110, 34);
    let last = rows.len().saturating_sub(1);
    for (i, line) in rows.iter().enumerate() {
        if i == last {
            continue;
        }
        let words = line.split_whitespace().count();
        assert!(words < 20, "a 25-word line would be a seed: {line}");
    }
}

// ======================================================================================
// The TUI UX fix-up round: DEFECT A (the tab strip's hit rects), DEFECT B (the picker's
// address budget), DEFECT C (the `?` overlay's first row) and FINDING D (the wallet badge).
// ======================================================================================

/// The column at which `needle` is drawn in `haystack`, in **cells** rather than bytes: the
/// strip can carry a spinner or a nettype, and a multi-byte char would make byte offsets lie.
fn cell_col(haystack: &str, needle: &str) -> Option<u16> {
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    let at = (0..=h.len().saturating_sub(n.len())).find(|&i| h[i..i + n.len()] == n[..])?;
    u16::try_from(at).ok()
}

/// **The assertion the whole mouse suite was missing** (DEFECT A). Every other mouse case reads
/// its coordinate out of `m.hits` — the same map it then clicks — so a map two columns off the
/// rendered text is perfectly self-consistent and passes: the suite can prove routing and can
/// never prove alignment. This one reads the **frame**. It finds each tab's title in the strip
/// that was actually drawn, clicks the title's LAST cell and the next tab's FIRST cell, and
/// asserts where it lands.
///
/// On the pre-fix tree the last cell of `2 Wallet` selected Send, of `3 Send` selected Log, and
/// `4 Log` selected nothing at all, because `tab_strip` advanced by the bare title width while
/// `Tabs::tab_label` draws `title.width() + 2`.
#[test]
fn a_click_on_a_tab_label_lands_on_that_tab_and_never_on_its_neighbour() {
    for (w, h) in [(150u16, 40u16), (100, 30), (90, 30), (56, 24)] {
        // Below 60 columns or 20 rows `tab_strip` draws its own `1·2·3·4` (§7a resize); above,
        // it delegates to `Tabs`, which is the tier the defect was in.
        let narrow = w < 60 || h < 20;
        let titles: [&str; 4] = if narrow {
            ["1", "2", "3", "4"]
        } else {
            ["1 Node", "2 Wallet", "3 Send", "4 Log"]
        };
        for (i, title) in titles.iter().enumerate() {
            // Start two screens away, so that neither the tab under test nor the neighbour a
            // misaligned rect selects is where we already are: "nothing happened" must fail.
            let from = Screen::from_index(i + 2);
            let mut s = sim(offline_model(from));
            let strip = render(s.model(), w, h).remove(0);
            let at = cell_col(&strip, title).unwrap_or_else(|| {
                panic!("`{title}` is not in the strip drawn at {w}x{h}: {strip:?}")
            });
            let last = at + title.chars().count() as u16 - 1;
            s.inject_event(click(last, 0));
            assert_eq!(
                s.model().screen,
                Screen::from_index(i),
                "a click on column {last}, the LAST cell of `{title}` at {w}x{h} (from {from:?}), \
                 in the strip {strip:?}"
            );
            if i + 1 >= titles.len() {
                continue;
            }
            // And the FIRST cell of the next tab is the next tab's: at the wide tiers that is
            // its leading decoration cell, two columns left of its title (the `[`/space `Tabs`
            // draws, plus the literal's own leading space).
            let next = titles[i + 1];
            let nat = cell_col(&strip, next).expect("the next title");
            let first = if narrow { nat } else { nat - 2 };
            let mut s2 = sim(offline_model(Screen::from_index(i + 3)));
            let _ = render(s2.model(), w, h);
            s2.inject_event(click(first, 0));
            assert_eq!(
                s2.model().screen,
                Screen::from_index(i + 1),
                "a click on column {first}, the FIRST cell of `{next}` at {w}x{h}"
            );
        }
    }
}

/// §7a.4: the **address** is "never elided, for the reason the send confirm never elides an
/// address". DEFECT B: at the brief's own 150×40 the owner's configured node rendered as
/// `● xmr-node.cakewallet.co` while a 100-column terminal rendered it whole — the drop loop
/// budgeted `COL` per kept column and `Table` spends `COL + 1`, and its 26-column minimum was
/// below the 31 cells that address needs. The list carries the longest address monero.fail
/// actually served this machine (49 columns) so the cap is exercised and not assumed.
#[test]
fn the_picker_never_elides_the_configured_address_at_any_width() {
    const MINE: &str = "xmr-node.cakewallet.com:18081";
    const LONGEST: &str = "static.63.34.245.188.clients.your-server.de:18089";
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-addr-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = MINE.into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    seed_cache(
        &ctx,
        &[
            (MINE, false),
            (LONGEST, true),
            ("109.199.98.74:18081", true),
        ],
    );
    for w in [90u16, 100, 120, 150, 180] {
        let s = sim(model(&ctx, Screen::Node));
        let text = sim_text(&s, w, 40);
        assert!(
            text.contains("Public nodes"),
            "the picker must be on screen at {w} columns:\n{text}"
        );
        assert!(
            text.contains(&format!("● {MINE}")),
            "the configured address is elided at {w} columns:\n{text}"
        );
        // Wide enough to seat the longest row too, once the budget is right: half of the pane at
        // 180 is 54 cells and the widest cell is 51, so nothing there is cut either.
        if w == 180 {
            assert!(
                text.contains(LONGEST),
                "the widest address is elided at {w} columns:\n{text}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// DEFECT C, as the one test that reads the overlay's text **and** the dispatcher's behaviour:
/// `KEYS`'s `always` row may only name keys that are resolved above every branch, and the row
/// that names `1 2 3 4` has to say what the Send screen's own status line already says.
///
/// The old first row — `Tab  ⇧Tab  1 2 3 4` → "screens — always, even inside a field or a
/// modal" — was false the moment a field had focus, which on the money screen means a digit
/// lands in the amount.
#[test]
fn the_keys_table_promises_always_only_for_the_keys_the_dispatcher_resolves_first() {
    let always: Vec<&(&str, &str)> = KEYS
        .iter()
        .filter(|(_, what)| what.contains("Always"))
        .collect();
    assert_eq!(
        always.len(),
        1,
        "exactly one row may claim `always`, and it is §7a.1's: {always:?}"
    );
    assert_eq!(
        always[0].0, "Tab  ⇧Tab  Ctrl+C",
        "`always` may name only the keys resolved above every screen, focus state and modal"
    );
    let numbers = KEYS
        .iter()
        .find(|(k, _)| k.contains("1 2 3 4"))
        .expect("a row for the direct route");
    assert!(
        numbers.1.contains("no field has focus"),
        "the `1 2 3 4` row has to carry its condition: {numbers:?}"
    );
    // Rendered, because the overlay is the thing a user reads.
    let mut m = offline_model(Screen::Node);
    m.show_help = true;
    let text = screen_text(&m, 110, 44);
    assert!(
        !text.contains("even inside a field"),
        "the overlay still renders the sentence that is false:\n{text}"
    );
    assert!(text.contains("when no field has focus"), "{text}");

    // The behaviour, on the money screen with real digits — the capture that found it.
    let mut s = sim(offline_model(Screen::Send));
    s.inject_event(key(KeyCode::Down)); // To → Amount (§7a.1: fields move with ↑↓)
    type_str(&mut s, "0.01");
    for c in ['2', '4'] {
        s.inject_event(key(KeyCode::Char(c)));
    }
    assert_eq!(
        s.model().screen,
        Screen::Send,
        "a focused field keeps every printable character, digits included"
    );
    assert_eq!(
        s.model().send.amount.value(),
        "0.0124",
        "the digits went into the amount, which is why the row cannot say `always`"
    );
    // `q` likewise: it is typed, not obeyed.
    s.inject_event(key(KeyCode::Char('q')));
    assert_eq!(s.model().send.amount.value(), "0.0124q");
    // Unfocused, the same keys are the direct route the row describes.
    s.inject_event(key(KeyCode::Escape));
    s.inject_event(key(KeyCode::Char('2')));
    assert_eq!(s.model().screen, Screen::Wallet);
    // And the `always` row is true from inside a focused field, which is the half that was.
    let mut s2 = sim(offline_model(Screen::Send));
    type_str(&mut s2, "9");
    s2.inject_event(key(KeyCode::Tab));
    assert_eq!(s2.model().screen, Screen::Log);
    let mut s3 = sim(offline_model(Screen::Send));
    type_str(&mut s3, "9");
    s3.inject_event(Event::Key(
        KeyEvent::new(KeyCode::Char('c')).with_modifiers(Modifiers::CTRL),
    ));
    assert_eq!(
        s3.model().exit.load(Ordering::SeqCst),
        130,
        "Ctrl+C is resolved before every branch, a focused field included"
    );
}

/// FINDING D: `wallet down` is an assertion about a daemon, and it was printed while the TUI's
/// own action was the reason the poll could not answer — two lines under a panel reading
/// `starting…`. The badge now has a term for it, and `down` survives only for the state it
/// describes: nothing of ours in flight, and the poll still failing.
#[test]
fn the_wallet_badge_never_says_down_for_a_daemon_the_tui_is_itself_working() {
    // v5.6.1, V13: row 2 was `(true, None, "not running")`, and that is the contradiction the
    // verification caught — the Wallet screen said `wallet service: running` four rows above it.
    // A service that is up and silent is `not answering`; `not running` is kept for a service that
    // is not running.
    for (active, action, want) in [
        (false, None, "not running"),
        (true, None, "not answering"),
        // **S9: one word while fmonero is the reason the poll cannot answer.** It said
        // `starting` for a start and `scanning` for a CREATE, in a frame whose status bar and
        // whose Wallet pane both named the create — three descriptions of one thing.
        (false, Some(subs::Action::WalletDaemonStart), "working"),
        (
            true,
            Some(subs::Action::WalletCreate {
                name: "fresh".into(),
                password: "x".into(),
                language: "English".into(),
            }),
            "working",
        ),
    ] {
        let dir = std::env::temp_dir().join(format!(
            "fmonero-tui-badge-{}-{active}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = Context::for_test(&dir).expect("test context");
        // A port nothing listens on, so every poll fails; the §2i fixture decides whether the
        // unit is active, which is the other half of the state the badge reads.
        ctx.settings.wallet.url = "http://127.0.0.1:2".into();
        ctx.settings.node.url = "http://127.0.0.1:1".into();
        let fixture = dir.join("systemctl.fixture");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            &fixture,
            format!(
                "fmonero-monerod.service inactive disabled yes -\n\
                 fmonero-wallet-rpc.service {} disabled yes -\n",
                if active { "active" } else { "inactive" }
            ),
        )
        .expect("fixture");
        ctx.systemctl = fmonero_core::context::Systemctl::Fixture(fixture);
        let mut s = sim(model(&ctx, Screen::Wallet));
        assert_eq!(
            s.model().status.as_ref().map(|st| st.wallet.unit_active),
            Some(active),
            "the fixture must really reach unit_active = {active}, or this proves nothing"
        );
        assert!(
            !s.model().has_warning("wallet_idle"),
            "local mode with no wallet is `wallet_down`, not `wallet_idle` — the arm above"
        );
        if let Some(a) = action {
            let _ = s.model_mut().start(a);
            assert!(s.model().action.is_some(), "the action must be armed");
        }
        assert_eq!(s.model().wallet_badge(), want, "unit_active = {active}");
        let text = sim_text(&s, 120, 40);
        assert!(text.contains(&format!("wallet {want}")), "{text}");
        if want != "not running" {
            assert!(
                !text.contains("wallet not running"),
                "the status line still says not running:\n{text}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- §7a.7: settings that change under a running TUI (v5.6) --------------------------

/// The owner: *"When I change nodes it says I'm still on Cake Wallet."*
///
/// `Tui` held **one** `Context`, cloned at startup, and never reloaded it, so after a switch
/// every poll asked `status` about the old `node.remote`, the hero printed the old address, the
/// status bar printed it too, and the picker's `●` stayed on the old row. The address was right
/// in `config.toml`, right in the unit and right in the wallet service's argv, and wrong in every
/// pixel.
///
/// Three cases, and each is a real sequence rather than a call of the fix: the switch the owner
/// made, a change from another terminal, and an unreadable file mid-session.
fn stale_node_ctx(tag: &str) -> (std::path::PathBuf, Context) {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-stale-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = NODE_A.into();
    // A port nothing listens on, so no wallet answers and the screen is the public-node one.
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    (dir, ctx)
}

const NODE_A: &str = "a.example:18081";
const NODE_B: &str = "b.example:18081";

/// Write `config.toml` the way `node_mode::set_remote` does — the single `Settings::save` of
/// §2m.5, which happens **before** the unit render, and is therefore exactly what is on disk
/// after a switch.
fn write_remote(ctx: &Context, address: &str) {
    let mut s = ctx.settings.clone();
    s.set("node.remote", address).expect("a valid address");
    s.set("node.mode", "remote").expect("remote");
    std::fs::create_dir_all(&ctx.paths.config_dir).expect("config dir");
    s.save(ctx).expect("save");
    // `stat` has one-second granularity on some filesystems and the model compares mtimes, so
    // the written file is aged deliberately rather than racing the test's own clock.
    let _ = std::fs::metadata(ctx.paths.config_file()).expect("the file exists");
}

/// A cached check of `address`, so §7a.3's pane has something to render for the new node —
/// which is the fourth surface that was wrong (§7a.7 part 3).
fn cache_check(ctx: &Context, address: &str, height: u64) {
    let probe = fmonero_core::nodes::Probe {
        reachable: true,
        height: Some(height),
        synchronized: Some(true),
        restricted: Some(false),
        nettype: Some("mainnet".into()),
        version: Some("0.18.5.1".into()),
        latency_ms: Some(240),
        reason: None,
        probed_at: fmonero_core::now_unix(),
    };
    state::note_remote_probe(ctx, address, &probe).expect("the cached check");
}

/// Case 1 — **the switch the owner made.** All four surfaces name the new node within one poll.
#[test]
fn switching_nodes_moves_every_surface_within_one_poll() {
    let (dir, ctx) = stale_node_ctx("switch");
    write_remote(&ctx, NODE_A);
    seed_cache(&ctx, &[(NODE_A, false), (NODE_B, false)]);
    cache_check(&ctx, NODE_B, 3_765_346);
    let mut s = sim(model(&ctx, Screen::Node));

    // Before: every surface names A.
    let before = sim_text(&s, 120, 44);
    assert!(
        before.contains(NODE_A),
        "the fixture starts on A:\n{before}"
    );

    // The switch: `config.toml` now holds B — which is what `node_mode::set_remote` writes
    // before anything else (§2m.5) — and the action reports the outcome it reports on a real
    // machine. `set_remote` itself is not called here: under `FMONERO_STATE_DIR` it refuses at
    // `units::install` with exit 3 `sandboxed_units` **after** having saved the settings
    // (DECISIONS, 2026-09-18), so calling it would test the sandbox guard rather than the fix.
    write_remote(&ctx, NODE_B);
    s.send(Msg::ActionDone(Box::new(Ok(subs::Outcome::NodeSwitched {
        address: NODE_B.to_string(),
        must_reopen: None,
    }))));
    s.send(Msg::Poll);

    let text = sim_text(&s, 120, 44);
    // 1. the model's own `Context` — the thing that was stale.
    assert_eq!(
        s.model().ctx.settings.node.remote,
        NODE_B,
        "the reload did not happen"
    );
    // 2. the Node hero and 3. the status bar, both of which read the poll's Status document.
    assert!(
        text.contains(&format!("Public node · synced · {NODE_B}")),
        "the hero still names the old node:\n{text}"
    );
    assert!(
        text.contains(&format!("● synced {NODE_B}")),
        "the status bar still names the old node:\n{text}"
    );
    // 4. the picker's `●`, which compares each row against the address in force.
    assert!(
        text.contains(&format!("● {NODE_B}")),
        "the picker's marker did not move:\n{text}"
    );
    assert!(
        !text.contains(&format!("● {NODE_A}")),
        "the marker is on two rows:\n{text}"
    );
    // 5. and ` This node ` shows B's measurement rather than `Not checked yet.`
    assert!(
        text.contains("block 3 765 346"),
        "the pane must show the new node's own check:\n{text}"
    );
    assert!(!text.contains("Not checked yet."), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Case 2 — **a change made anywhere else**: the CLI in another terminal, an editor, a
/// `doctor --fix` that repaired a key. Part 1 cannot see those, and part 2 is the `stat` on the
/// 1 s poll that does.
#[test]
fn a_settings_change_from_outside_is_noticed_on_the_next_poll() {
    let (dir, ctx) = stale_node_ctx("outside");
    write_remote(&ctx, NODE_A);
    seed_cache(&ctx, &[(NODE_A, false), (NODE_B, false)]);
    cache_check(&ctx, NODE_B, 3_765_346);
    let mut s = sim(model(&ctx, Screen::Node));
    assert!(sim_text(&s, 120, 44).contains(NODE_A));

    // Nothing in the TUI did this, and no action completed.
    write_remote(&ctx, NODE_B);
    s.send(Msg::Poll);

    let text = sim_text(&s, 120, 44);
    assert_eq!(s.model().ctx.settings.node.remote, NODE_B);
    assert!(
        text.contains(&format!("Public node · synced · {NODE_B}")),
        "the hero still names the old node:\n{text}"
    );
    // X13: the badge carries the verdict too, from the same predicate.
    assert!(text.contains(&format!("● synced {NODE_B}")), "{text}");
    assert!(text.contains("block 3 765 346"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Case 3 — **the failure path.** An unreadable `config.toml` mid-session leaves the previous
/// values on screen and raises **one** toast: a TUI that exited on an unreadable settings file
/// would be worse than one showing a stale address it tells you about (§7a.7 part 1).
#[test]
fn an_unreadable_settings_file_keeps_the_old_values_and_says_so_once() {
    let (dir, ctx) = stale_node_ctx("unreadable");
    write_remote(&ctx, NODE_A);
    seed_cache(&ctx, &[(NODE_A, false)]);
    let mut s = sim(model(&ctx, Screen::Node));
    assert!(sim_text(&s, 120, 44).contains(NODE_A));

    std::fs::write(ctx.paths.config_file(), b"this is not toml = = =\n").expect("clobber");
    s.send(Msg::Poll);
    let text = sim_text(&s, 120, 44);
    assert_eq!(
        s.model().ctx.settings.node.remote,
        NODE_A,
        "the old Context must survive an unreadable file"
    );
    assert!(text.contains(NODE_A), "{text}");
    assert!(
        s.model().note.contains("Could not read your settings"),
        "{}",
        s.model().note
    );

    // And **once**: a second poll with the same broken file must not raise it again, because the
    // mtime has already been recorded. (Another note may well be there — the poll also arms
    // §2n.4's check of the configured node — so the assertion is about this message, not about
    // the line being empty.)
    s.model_mut().note.clear();
    s.send(Msg::Poll);
    assert!(
        !s.model().note.contains("Could not read your settings"),
        "the same failure was raised twice: {}",
        s.model().note
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ======================================================================================
// The plain-language round 3 (W1–W22): the findings the 2026-09-19 verification read off a
// real screen, each with the assertion that keeps it closed.
// ======================================================================================

/// **W21.** `FMONERO_STATE_DIR` redirected state, config and cache and left the **defaults** of
/// `node.data_dir` and `node.log_file` at `~/.bitmonero`, so a sandbox that did not set them
/// pointed at the owner's own chain folder. This repo's own verification opened the Log screen on
/// `/home/oliver/.bitmonero/bitmonero.log` twice, and the doctor's `disk_space` and
/// `node_data_dir_single_writer` probes measure the same two paths.
///
/// The assertion is on the **rendered screen**, not only on the setting: the Log screen's title
/// and its empty-state line are where a reader sees which file is open.
#[test]
fn a_sandboxed_log_screen_never_opens_the_machines_own_chain_folder() {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-w21-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    // Deliberately NOT overriding `node.log_file`: the default is the thing under test.
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    let home_chain = ctx.paths.home.join(".bitmonero");
    assert!(
        ctx.settings.node.data_dir.starts_with(&dir)
            && ctx.settings.node.log_file.starts_with(&dir),
        "the sandbox's own chain folder and log: {} / {}",
        ctx.settings.node.data_dir.display(),
        ctx.settings.node.log_file.display()
    );
    let mut s = sim(model(&ctx, Screen::Log));
    s.send(Msg::Tick);
    let text = sim_text(&s, 150, 40);
    assert!(
        text.contains("bitmonero.log"),
        "the Log screen names the file it opened:\n{text}"
    );
    assert!(
        !text.contains(&home_chain.display().to_string()),
        "a sandbox may never render the machine's own chain folder:\n{text}"
    );
    assert_eq!(
        s.model().logs.path,
        dir.join("bitmonero/bitmonero.log"),
        "the Log screen reads the sandbox's log"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **W2.** `u` and `d` start and stop a background service on this machine, and both confirms
/// were `y confirms · Esc cancels` and nothing else — identical, to the character, to the capture
/// V4 was written from. Read at both sizes, in local mode, so the assertion is what a person sees.
#[test]
fn the_start_and_stop_confirms_state_what_they_do() {
    let w = World::new("confirm-node-power");
    // `confirm` refuses unless this machine's node is fmonero's own (§2i), so the sandbox gets a
    // unit file of its own — in its OWN `systemd_user_dir`, never the machine's (§4.2).
    std::fs::create_dir_all(&w.ctx.paths.systemd_user_dir).expect("sandbox systemd dir");
    std::fs::write(
        w.ctx.paths.systemd_user_dir.join(&w.ctx.settings.node.unit),
        b"[Unit]\n",
    )
    .expect("a unit file to detect");
    for (k, want) in [
        (
            'u',
            vec![
                "Start your node",
                "fmonero starts your node on this machine.",
                "keeps its copy of the blockchain in",
                "120 GB",
                "the first sync takes hours",
                "y to confirm",
            ],
        ),
        (
            'd',
            vec![
                "Stop your node",
                "fmonero stops your node on this machine.",
                "nothing is lost",
                "cannot see new payments",
                "press u to start it again",
                "y to confirm",
            ],
        ),
    ] {
        let mut s = sim(model(&w.ctx, Screen::Node));
        s.inject_event(key(KeyCode::Char(k)));
        for (cols, rows) in [(120u16, 32u16), (150, 40)] {
            let text = sim_text(&s, cols, rows);
            for line in &want {
                assert!(
                    text.contains(line),
                    "{cols}x{rows}: the `{k}` confirm does not say {line:?}:\n{text}"
                );
            }
            // The chain folder is named, and it is the sandbox's own (W21).
            assert!(
                text.contains(
                    &w.ctx
                        .settings
                        .node
                        .data_dir
                        .display()
                        .to_string()
                        .chars()
                        .take(20)
                        .collect::<String>()
                ),
                "{cols}x{rows}: the confirm must name the chain folder:\n{text}"
            );
            // Nothing is clipped: every row of the overlay ends inside its own border.
            assert!(
                !text.contains('…'),
                "{cols}x{rows}: something in the confirm was elided:\n{text}"
            );
        }
    }
}

/// **W9.** One predicate for the node's three states, on every surface that renders one.
///
/// The frame the verification read said `● Your node is not answering` over `Your node is not
/// running. Press u to start it.` while the status bar ran `⠸ starting the node 3s / 30s · Esc
/// cancels` — three claims about one node, and the middle one advising the key that was already
/// running. `Tui::node_off_words` is the predicate; core's `node_off_words` is the half the Notes
/// row reads.
#[test]
fn the_node_has_one_set_of_words_for_being_off_and_never_advises_the_running_key() {
    let mut s = sim(offline_model(Screen::Node));
    assert_eq!(s.model().node_off_words(), "not running");
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cols, rows);
        assert!(
            text.contains("Your node is not running"),
            "{cols}x{rows}:\n{text}"
        );
        assert!(
            !text.contains("not answering"),
            "{cols}x{rows}: two words for one state in one frame:\n{text}"
        );
    }
    // While `u` runs: the hero says so, and nothing in the frame advises pressing it.
    s.model_mut().start_for_test(subs::Action::NodeUp);
    assert_eq!(s.model().node_off_words(), "starting");
    for (cols, rows) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cols, rows);
        assert!(
            text.contains("Starting your node…"),
            "{cols}x{rows}: the hero must say what is happening:\n{text}"
        );
        assert!(
            !text.contains("Press u to start it"),
            "{cols}x{rows}: the hero advised the key that is already running:\n{text}"
        );
        assert!(
            !text.contains("Your node is not running"),
            "{cols}x{rows}: it is starting, not stopped:\n{text}"
        );
    }
}

/// **W15.** Every toast is a capitalised sentence.
///
/// `the wallet service is stopped` was the one the verification read, lowercase, beside a `[W]`
/// chip that said `Start` — but it was one of a dozen: `following`, `sent · <64 hex characters>`,
/// `checked 8 · 3 answered`, `select a node first (↑↓ or click), then Enter`. The literals are read
/// out of `mod.rs` and `subs.rs` rather than listed here, because a list beside 3 000 lines of
/// event handling is a list that goes stale — and the same scan covers the action labels, which
/// are the first toast of every long action.
#[test]
fn every_toast_is_a_capitalised_sentence() {
    fn literals(src: &str, needles: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for needle in needles {
            for (i, _) in src.match_indices(needle) {
                let rest = &src[i + needle.len()..];
                // The first string literal after the call, `format!(` and whitespace aside.
                let Some(open) = rest.find('"') else { continue };
                let head = &rest[..open];
                if head
                    .chars()
                    .any(|c| !c.is_whitespace() && !"(format!&".contains(c))
                {
                    continue;
                }
                let mut lit = String::new();
                let mut chars = rest[open + 1..].chars();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            // A continued literal: `\` then a newline and indentation.
                            match chars.next() {
                                Some('\n') => {
                                    lit.push(' ');
                                }
                                Some(other) => lit.push(other),
                                None => break,
                            }
                        }
                        '"' => break,
                        c => lit.push(c),
                    }
                }
                out.push(lit);
            }
        }
        out
    }
    let mut all = literals(
        include_str!("mod.rs"),
        &["self.note(", "Outcome::Note(", "Ok(\""],
    );
    all.extend(literals(
        include_str!("subs.rs"),
        &["Outcome::Note(", "=> \"", "=> format!("],
    ));
    assert!(
        all.len() > 30,
        "the scan found only {} literals — it is not reading the sources",
        all.len()
    );
    for lit in &all {
        let trimmed = lit.trim_start();
        let Some(first) = trimmed.chars().next() else {
            continue;
        };
        // A literal that opens with a format field starts with the value, which may be a wallet
        // name or an address; a debug-only or non-prose literal has no space in it at all.
        if first == '{' || !trimmed.contains(' ') && !trimmed.ends_with('.') {
            continue;
        }
        // The program's own name is lower case everywhere it appears, `fmonero:` in the CLI
        // included, so a sentence that opens with it is capitalised correctly (round 4
        // iteration 3, where the failed-action toasts became `fmonero could not …`).
        assert!(
            first.is_uppercase() || first.is_ascii_digit() || trimmed.starts_with("fmonero "),
            "a toast has to be a capitalised sentence: {lit:?}"
        );
    }
}

/// **W14.** The refusal the whole of §2o.1 is built on has to be readable: it was cut to
/// `You are using a public node. Press…` at 120 **and** 150 columns, so the two in-app routes it
/// exists to name were never on screen. The status bar's left segment carries two badges and the
/// node's address, and what is left for a toast is about 44 columns.
#[test]
fn the_public_node_refusal_names_both_routes_on_screen() {
    let w = World::new("refusal-toast");
    let mut ctx = w.ctx.clone();
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = "xmr-node.cakewallet.com:18081".into();
    let mut s = sim(model(&ctx, Screen::Node));
    // **X5: the situation first, and one refusal per key.** One 40-column toast served all three,
    // which is rule 11's hint standing in for the sentence — and a user who pressed `L` was told to
    // choose a node and never told why the log level could not be changed.
    for (k, want) in [
        ('u', "there is no node here to start"),
        ('d', "there is no node here to stop"),
        // M13: no shouted word — `OWN` was the only capitalised one in the product.
        ('L', "This sets how much your own node logs"),
    ] {
        s.inject_event(key(KeyCode::Char(k)));
        for (cols, rows) in [(120u16, 32u16), (150, 40)] {
            let text = sim_text(&s, cols, rows);
            let flat = flowed(&text);
            // On the bar when it fits beside the badges, on its own row when it does not — and
            // whole either way, which is the finding.
            assert!(
                flat.contains(want),
                "{cols}x{rows}: `{k}` must state its own situation:\n{text}"
            );
            assert!(
                flat.contains("Press l to use your own node")
                    || flat.contains("or press l to use your own"),
                "{cols}x{rows}: `{k}` must name the key that fixes it:\n{text}"
            );
            for line in text.lines().filter(|l| l.contains("public node")) {
                assert!(
                    !line.contains('…'),
                    "{cols}x{rows}: the refusal was cut: {line}"
                );
            }
        }
    }
}

/// **W16.** `Esc` with nothing of fmonero's own running said `Nothing is running.` on a screen
/// whose subject is a node and a wallet service, both of which a reader will think it means.
#[test]
fn esc_with_nothing_running_talks_about_the_cancel_and_not_about_the_node() {
    let mut s = sim(offline_model(Screen::Node));
    s.inject_event(key(KeyCode::Escape));
    assert_eq!(s.model().note, "Nothing to cancel.");
    assert!(!s.model().note_err, "it is not an error, it is an answer");
}

/// **W15.** The `[W]` chip says what `W` will do, from the predicate `W` itself branches on. In
/// the state §7a.5 describes — the service up, nothing open — the chip read `Start` and pressing
/// it stopped the service, and then toasted about it in lower case.
#[test]
fn the_wallet_service_chip_says_which_way_the_key_goes() {
    let mut s = sim(offline_model(Screen::Wallet));
    // Nothing running: `W` starts it — and **no chip is drawn**, because opening a wallet starts
    // the service and the chip's own label used to say exactly that (round 4 iteration 3). The key
    // still works and the `?` overlay still explains it.
    assert!(!s.model().wallet_daemon_key_stops());
    assert!(s.model().wallet_daemon_chip().is_none());
    let text = sim_text(&s, 120, 32);
    assert!(
        !text.contains("[W] Start the wallet service") && !text.contains("[W] Stop the wallet"),
        "the chip cannot act here, so it is not drawn:\n{text}"
    );
    // The service up with nothing open — §7a.5's stop state, reached by moving the one field the
    // predicate reads, because no wallet mock answers "running with nothing open".
    if let Some(st) = s.model_mut().status.as_mut() {
        st.wallet.unit_active = true;
        st.wallet.open = false;
        st.wallet.name = None;
    }
    assert!(s.model().wallet_daemon_key_stops());
    let text = sim_text(&s, 120, 32);
    assert!(
        text.contains("[W] Stop the wallet service")
            && !text.contains("[W] Start the wallet service"),
        "with the service up and nothing open the chip stops it:\n{text}"
    );
}

// ---- round 5: the twelve serious findings, each with the state that reproduced it ---------

/// **S1: the picker cuts a node address mid-token at 120 columns, with no marker.**
///
/// `screens.rs:869` capped the address column at half the pane — `widest.min(rows[0].width / 2)` —
/// so at 120 columns, where the picker's pane is 72 wide, the budget was 36 rather than the measured
/// 46, three data columns were kept, and `Table` truncated
/// `monero-rpc.cheems.de.box.skhron.com.ua:18089` to `…skhron.com.ua:180`: no ellipsis, no marker,
/// and `:180` is a plausible port to type into `[a] Add a node`. §7a.4 says the address is never
/// elided at any tier, and `screens.rs:846` says it too.
///
/// The fixture is the widest address `tests/fixtures/nodes/nodes.json` actually carries (44
/// characters) plus the 43-character one the round asked for, and the assertion is character
/// equality against the whole cell at every width from 90 up.
#[test]
fn the_picker_renders_every_address_whole_at_every_width() {
    const MINE: &str = "xmr-node.cakewallet.com:18081";
    // 43 characters, as the round asked for.
    const FORTY_THREE: &str = "monero-rpc.cheems.de.box.skhron.com.u:18089";
    // 44: the widest monero.fail has ever served this machine, from the fixture.
    const WIDEST: &str = "monero-rpc.cheems.de.box.skhron.com.ua:18089";
    assert_eq!(FORTY_THREE.chars().count(), 43);
    assert_eq!(WIDEST.chars().count(), 44);
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-picker-whole-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.remote = MINE.into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    seed_cache(&ctx, &[(MINE, false), (FORTY_THREE, true), (WIDEST, true)]);
    // Height 44, because the picker's table needs rows to draw one: at 90x32 the pane has room for
    // its header, its footer and its chips and nothing else, and a table with no data row proves
    // nothing about an address. The widths are the two the round captured plus the three tiers
    // around them.
    for w in [90u16, 100, 120, 150, 180] {
        let s = sim(model(&ctx, Screen::Node));
        let text = sim_text(&s, w, 44);
        assert!(
            text.contains("Public nodes"),
            "the picker must be on screen at {w} columns:\n{text}"
        );
        for address in [MINE, FORTY_THREE, WIDEST] {
            assert!(
                text.contains(address),
                "the address {address} is cut at {w} columns:\n{text}"
            );
        }
        // The configured row keeps its marker, and the marker is part of the widest cell.
        assert!(text.contains(&format!("● {MINE}")), "{text}");
        // And nothing in the table is elided: the picker has no ellipsis of its own, unlike the
        // footer, whose counts it wraps.
        for line in text.lines().filter(|l| l.contains(".com")) {
            assert!(
                !line.contains('…'),
                "an address was elided at {w}: {line}\n{text}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **S3: `list_modal` deleted one character from every line it filled.**
///
/// `text_w` was `MODAL_W - 4` = 64, `overlay`'s interior is also `MODAL_W - 4` = 64, and each line is
/// drawn `format!(" {part} ")` — `part` plus two — into a `Paragraph` with `WrapMode::None`, which
/// truncates at 64. So every line that filled the budget lost exactly its last character: a
/// 248-character wallet path came out as 245, with a `-`, a `0` and another `-` gone from the middle,
/// and the open-a-wallet modal named a folder that does not exist. `wrap_hanging` was lossless all
/// along; the budget was two columns wider than the place the text goes.
#[test]
fn the_open_wallet_modal_renders_a_248_character_path_character_complete() {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-modalpath-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    // The exact shape the capture found: a scratch path 248 characters long, with no wallets in it,
    // so the modal's one row IS that path.
    let long: PathBuf = PathBuf::from(format!(
        "/tmp/claude-1000/-home-oliver--config-Claude-scratch-workspaces-{}/{}/scratchpad/r5/sb-A/data/fmonero/wallets/mainnet",
        "eff1ae0e-d485-4f42-9ffb-620b5efc18d5-813ee62c-1eed-4c69-bc1f-b109cdbca45",
        "scratch-2026-09-16-3ded94/4901fef8-2906-4bc0-b226-799715c2e661"
    ));
    let path = long.display().to_string();
    assert!(
        path.chars().count() >= 240,
        "{} chars",
        path.chars().count()
    );
    ctx.settings.wallet.dir = long;
    let mut m = model(&ctx, Screen::Wallet);
    let _ = m.on_key(KeyEvent::new(KeyCode::Char('o')));
    for (w, h) in [(120u16, 32u16), (150, 40)] {
        let text = screen_text(&m, w, h);
        // Every character of the path, in order, with the wrapping and the modal's own padding
        // removed — which is what `flowed` does, and the hanging indent is spaces.
        let flat: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '│' && *c != '╭' && *c != '╮')
            .collect();
        let want: String = path.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            flat.contains(&want),
            "the modal lost characters of the path at {w}x{h}.\nwant: {want}\ngot:  {flat}\n{text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **S6: the privacy consent modal had no visible control, at both sizes.**
///
/// `overlay(m, frame, 72, 10, …)` leaves `inner.height = 6`, and the six logical lines need **seven**
/// rows because the third sentence is 96 characters in a 68-column interior and wraps. So the one
/// screen in the product that asks permission to reveal the owner's IP address to a stranger blocked
/// the screen, stated neither the way forward nor the way back, and the unlabelled key that
/// dismisses it — `Enter` — is the one that consents and writes `notice_ack`.
/// `grep -c "Enter continues"` over all seventy-eight captures of round 4 returned **0**.
#[test]
fn the_privacy_notice_shows_its_keys_at_both_sizes() {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-notice-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.mode = fmonero_core::NodeMode::Remote;
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    // No cache at all, so `f` is the state §2n.5 rule 2 puts the notice in front of — which is the
    // owner's own cold-cache state today.
    let mut m = model(&ctx, Screen::Node);
    let _ = m.on_key(KeyEvent::new(KeyCode::Char('f')));
    for (w, h) in [(120u16, 32u16), (150, 40)] {
        let text = screen_text(&m, w, h);
        assert!(
            text.contains("Before contacting anyone"),
            "the notice is not up at {w}x{h}:\n{text}"
        );
        for needle in [
            "Getting the list tells monero.fail your IP address.",
            "Enter continues · Esc cancels",
        ] {
            assert!(
                flowed(&text.replace('│', " ")).contains(needle),
                "{w}x{h} lost `{needle}`:\n{text}"
            );
        }
        // And the last sentence is whole: it is the one that wraps, and the row it needed was the
        // row the keys lost.
        assert!(
            flowed(&text.replace('│', " ")).contains("not a recommendation, and not yours."),
            "{w}x{h} cut the third sentence:\n{text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **S8: the Send screen refused while a payment was pending and named no way out.**
///
/// A filled, valid form plus `Enter` gave one line — *"An earlier payment may still be on its way…"*
/// — with no key, no chip and no next step (§2o rule 11). The only resolution in the product was
/// `fmonero wallet clear-pending --yes`, which `docs/COPY.md:151` had deliberately removed from the
/// banner above the form, which is not in §2o.1's exhaustive in-app list and not one of its four
/// `In a terminal:` exceptions, and which was named on **no** TUI surface. So a person was left on a
/// money screen that would not build, with the obvious next move being to send again.
#[test]
fn a_pending_record_can_be_resolved_from_the_wallet_screen() {
    let w = World::with_wallet("s8-clear-pending", &[]);
    let record = PendingSend {
        kind: PendingKind::Send,
        account_index: 0,
        dest: DEST.to_string(),
        built_at: 1_789_000_000,
        txs: vec![PendingTx {
            txid: BUILT_TXID.to_string(),
            amount: fmonero_core::Piconero(100_000_000_000),
            fee: fmonero_core::Piconero(30_720_000),
            relayed: false,
            relayed_at: None,
        }],
    };
    state::set_pending_send(&w.ctx, &record).expect("the record");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    assert!(s.model().has_pending(), "the record is in `status`");
    // The chip names the key, at the owner's size.
    let text = sim_text(&s, 120, 32);
    assert!(
        text.contains("[P] Clear that record"),
        "the chip is missing:\n{text}"
    );
    // `P` asks first, and the confirm states the risk in plain words rather than asking for a `y`
    // against a bare fact.
    s.inject_event(key(KeyCode::Char('P')));
    let modal = screen_text(s.model(), 120, 32);
    let flat = flowed(&modal.replace('│', " "));
    assert!(flat.contains("Clear that record"), "{modal}");
    for needle in [
        "does not know whether",
        "does not cancel anything",
        "will not let you send again",
        "stops you paying twice",
        "look at your payments first",
        "y to confirm · Esc cancels",
    ] {
        assert!(
            flat.contains(needle),
            "the confirm lost `{needle}`:\n{modal}"
        );
    }
    // And no shell command reaches it (§2o.1).
    assert!(!flat.contains("fmonero wallet"), "{modal}");
    // Confirming runs the same core sequence `wallet clear-pending` runs, and the record is gone.
    s.inject_event(key(KeyCode::Char('y')));
    // `ProgramSimulator` runs `Cmd::task` synchronously, so the key press IS the clear.
    assert!(
        state::pending_send(&w.ctx).expect("readable").is_none(),
        "the record was not cleared"
    );
    // Esc instead of `y` leaves it alone, which is the half a confirm exists for.
    state::set_pending_send(&w.ctx, &record).expect("the record again");
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    s.inject_event(key(KeyCode::Char('P')));
    s.inject_event(key(KeyCode::Escape));
    assert!(state::pending_send(&w.ctx).expect("readable").is_some());
}

/// **S12: the help overlay said `Tab` always works in a pop-up; in the send confirm it did nothing
/// and said nothing.**
///
/// `switch_screen` raises `Press Esc first to discard this.`, and `screens::toast_hidden` suppresses
/// every toast while `m.send.stage` is `Stage::Built` — so the one modal that needed that sentence
/// was the one that could not show it. §7a.1 says the exception is documented *in the modal*, on the
/// line under its buttons: the create-wallet modal carries its version and the send confirm carried
/// nothing.
#[test]
fn tab_inside_the_send_confirm_says_which_key_comes_first() {
    let w = World::new("s12-tab-confirm");
    let mut s = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    s.init();
    type_str(&mut s, DEST);
    // `↑↓` move between fields; `Tab` is the key this test is about and must not be used to reach
    // the amount (§7a.1).
    s.inject_event(key(KeyCode::Down));
    type_str(&mut s, "0.1");
    s.inject_event(key(KeyCode::Enter));
    run_action(&mut s);
    assert!(
        s.model().send.confirming(),
        "the transaction must be built for this state to exist"
    );
    let before = sim_text(&s, 120, 32);
    assert!(
        !before.contains("Esc first, then Tab"),
        "the notice is only shown once Tab is pressed:\n{before}"
    );
    s.inject_event(key(KeyCode::Tab));
    assert!(
        s.model().send.confirming(),
        "Tab may not leave a built transaction"
    );
    assert_eq!(s.model().screen, Screen::Send, "and may not switch screens");
    for (cw, ch) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cw, ch);
        assert!(
            flowed(&text.replace('│', " ")).contains("Esc first, then Tab."),
            "{cw}x{ch}: the modal said nothing about Tab:\n{text}"
        );
        // The confirm word's own line is still on screen: the notice is a row the modal grew, not a
        // row it took (§7a: the safety surface is never abbreviated).
        assert!(
            flowed(&text.replace('│', " ")).contains("and press Enter to send"),
            "{cw}x{ch}: the confirm prompt was pushed off:\n{text}"
        );
    }
    // Esc discards, and the notice goes with the confirm.
    s.inject_event(key(KeyCode::Escape));
    assert!(!s.model().send.confirming());
    assert!(!s.model().send.tab_note);
}

/// **S9: the payments ledger showed no year, anywhere in the TUI, at any width.**
///
/// `short_time` returned `MM-DD HH:MM` unconditionally — in the list, in the `Enter` detail view and
/// in the note modal's own title — while `fmonero wallet history` two keys away printed
/// `2026-09-16 14:01`. A payment from September 2024 and one from September 2026 were the same row on
/// the screen the owner reads to reconcile what he was paid.
#[test]
fn the_payments_ledger_carries_the_year_at_the_owners_widths() {
    let w = World::with_wallet("s9-year", &[]);
    let mut s = sim(model(&w.ctx, Screen::Wallet));
    let rows = s.model().wallet.rows.clone();
    let first = rows.first().expect("the fixture has payments").clone();
    let stamp = fmonero_core::when(first.timestamp).expect("a real instant");
    assert_eq!(stamp.chars().count(), 16, "{stamp}");
    for (cw, ch) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, cw, ch);
        assert!(
            text.contains(&stamp),
            "{cw}x{ch}: the list has no year:\n{text}"
        );
        // The header is sized for it, so no row is pushed out of the pane.
        assert!(text.contains("WHEN "), "{text}");
        // M6's note column survives the five extra cells at 120 (`history_cols` writes out why).
        assert!(
            text.contains("NOTE"),
            "{cw}x{ch}: the note column went:\n{text}"
        );
    }
    // The detail view and the note modal title, which are the other two readings of the same fact.
    s.inject_event(key(KeyCode::Enter));
    let detail = sim_text(&s, 120, 32);
    assert!(
        flowed(&detail.replace('│', " ")).contains(&stamp),
        "the payment detail has no year:\n{detail}"
    );
    s.inject_event(key(KeyCode::Escape));
    s.inject_event(key(KeyCode::Char('n')));
    let note = sim_text(&s, 120, 32);
    assert!(
        note.contains(&stamp),
        "the note modal's title has no year:\n{note}"
    );
}

/// **M10: the Log screen names its file, and a title cannot wrap.**
///
/// The title read `╭ Log  follow on  /tmp/claude-1000/…-620b5efc18d… ╮` at both sizes — the path
/// elided mid-token, in the only place in the product the file is named, and `fit` is right for a
/// table cell and wrong for the one string a reader may need to type. The title keeps the path when
/// it fits whole and the pane's own first row carries it, hard-chunked, when it does not.
#[test]
fn the_log_screen_names_its_file_whole_at_every_width() {
    let dir = std::env::temp_dir().join(format!(
        "fmonero-tui-logpath-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut ctx = Context::for_test(&dir).expect("test context");
    ctx.settings.node.url = "http://127.0.0.1:1".into();
    ctx.settings.wallet.url = "http://127.0.0.1:2".into();
    // A path no title can hold: 168 characters, the shape a scratch workspace produces.
    let long = dir.join(format!(
        "{}/bitmonero.log",
        "a-very-long-workspace-segment".repeat(4)
    ));
    std::fs::create_dir_all(long.parent().expect("a parent")).expect("dirs");
    std::fs::write(&long, b"2026-09-16 14:02:11 INFO synced\n").expect("a log");
    ctx.settings.node.log_file = long.clone();
    let path = long.display().to_string();
    assert!(path.chars().count() > 150, "{}", path.chars().count());
    let s = sim(model(&ctx, Screen::Log));
    for (w, h) in [(120u16, 32u16), (150, 40)] {
        let text = sim_text(&s, w, h);
        // Whole, in order, with the wrapping and the pane's chrome taken out.
        let flat: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '│' && *c != '╭' && *c != '╮' && *c != '─')
            .collect();
        assert!(
            flat.contains(&path),
            "{w}x{h} lost part of the log path:\n{text}"
        );
        // And nothing in the frame is elided: `fit`'s ellipsis is what this replaced.
        assert!(
            !text.contains('…'),
            "{w}x{h} still elides something:\n{text}"
        );
        // A short path goes back in the title, which is where one fact belongs when it fits.
        assert!(text.contains("Log  follow"), "{text}");
    }
    let mut short = ctx.clone();
    short.settings.node.log_file = dir.join("bitmonero.log");
    std::fs::write(&short.settings.node.log_file, b"x\n").expect("a log");
    let s = sim(model(&short, Screen::Log));
    let text = sim_text(&s, 150, 40);
    let title = text.lines().nth(1).unwrap_or_default().to_string();
    assert!(
        title.contains(&short.settings.node.log_file.display().to_string()),
        "a path that fits belongs in the title: {title}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **M13 and M14: the empty wallet panel says there are none, and every fact on it names a step.**
///
/// With no wallets the `[o]` chip is not drawn — `o` has nothing to open — and the comment at
/// `screens.rs:1861` claimed "the line under it says there is nothing yet" while the two rows that
/// would have carried such a line were discarded with `let _ = (rows[1], rows[3]);`. So the panel
/// said nowhere that the folder is empty. And its two status rows stated a fact and offered nothing:
/// §2o rule 11 unmet twice, on the screen whose whole content is controls.
#[test]
fn the_empty_wallet_panel_says_there_are_none_and_every_row_names_a_step() {
    let mut s = sim(offline_model(Screen::Wallet));
    s.model_mut().wallets = Vec::new();
    for (w, h) in [(120u16, 32u16), (150, 40), (70, 24)] {
        let text = sim_text(&s, w, h);
        let flat = flowed(&text.replace('│', " "));
        assert!(flat.contains("No wallet is open."), "{w}x{h}:\n{text}");
        // M13.
        assert!(
            flat.contains("There are no wallets on this machine yet."),
            "{w}x{h} says nowhere that the folder is empty:\n{text}"
        );
        // And `[o]` is still not drawn, because it still cannot help.
        assert!(!text.contains("[o] Open a wallet"), "{w}x{h}:\n{text}");
        assert!(text.contains("[c] Create a wallet"), "{w}x{h}:\n{text}");
        // M14: both status rows.
        assert!(
            flat.contains("Opening or creating a wallet starts it"),
            "{w}x{h}: the service row offers nothing:\n{text}"
        );
        assert!(
            flat.contains("press 1 for the Node screen"),
            "{w}x{h}: the node row offers nothing:\n{text}"
        );
    }
    // With a wallet there, the sentence goes: it would be false.
    s.model_mut().wallets = vec!["moneroli".to_string()];
    let text = sim_text(&s, 120, 32);
    assert!(
        !text.contains("There are no wallets"),
        "the sentence must not survive a wallet appearing:\n{text}"
    );
    assert!(text.contains("[o] Open a wallet"), "{text}");
}
