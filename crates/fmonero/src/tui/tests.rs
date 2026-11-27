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

use super::send::{Stage, CONFIRM_WORD};
use super::{send, subs, text_input, Modal, Msg, Screen, Tui, KEYS};

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

fn test_context(dir: &std::path::Path) -> Result<Context, fmonero_core::Error> {
    let mut ctx = Context::for_test(dir)?;
    let fixture = dir.join("systemctl.txt");
    std::fs::write(&fixture, "").unwrap();
    ctx.systemctl = fmonero_core::Systemctl::Fixture(fixture);
    Ok(ctx)
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
        let mut ctx = test_context(&dir).expect("test context");
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

    fn open_fixture_wallet(&self) {
        self.ctx
            .settings
            .save(&self.ctx)
            .expect("save isolated mock endpoints");
        let w = fmonero_core::WalletClient::from_ctx(&self.ctx).unwrap();
        fmonero_core::wallet_session::open_secret(
            &self.ctx,
            &w,
            "main",
            fmonero_core::SecretString::empty(),
            &CancelToken::new(),
        )
        .expect("explicit mock wallet open through normal session API");
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
    let mut ctx = test_context(&dir).expect("test context");
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

fn modal_text(m: &Tui, width: u16, height: u16) -> String {
    let rows = render(m, width, height);
    let area = m
        .hits
        .modal_area()
        .expect("rendered modal has a hit region");
    rows.into_iter()
        .skip(area.y as usize)
        .take(area.height as usize)
        .map(|row| {
            row.chars()
                .skip(area.x as usize)
                .take(area.width as usize)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    for screen in Screen::ALL {
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
    for (cols, rows) in [(50, 16), (40, 10)] {
        let strip = render(&m, cols, rows).remove(0);
        assert!(strip.contains("▥  FMONERO"), "{strip}");
        assert!(flowed(&strip).contains("1 2 3 4 5 6"), "{strip}");
        assert!(strip.contains("▌ Wallet"), "{strip}");
        for (i, label) in ["1", "2", "3", "4", "5", "6"].into_iter().enumerate() {
            let x = cell_col(&strip, label).expect("visible destination");
            assert_eq!(m.hits.at(x, 0, None), Some(Target::Tab(i)));
        }
    }
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
    assert!(text.contains("(C)reate a wallet"), "{text}");
    assert!(text.contains("wallet service:"), "{text}");
    // **Only the chips that can act** (round 4 iteration 3). With no wallets on this machine `o`
    // has nothing to open and its label said so — `(none on this machine yet. Press c to create
    // one)` — and `W` said `(not needed: opening a wallet starts it)`. Two controls telling the
    // reader not to press them, on the screen whose whole content is controls. `Create a wallet`
    // is the one thing to do here, and both keys are still in the `?` overlay.
    assert!(
        !text.contains("(O)pen a wallet"),
        "with no wallets there is nothing for o to open:\n{text}"
    );
    assert!(
        !text.contains("(W) Start the wallet service"),
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
        (Screen::Log, Screen::Receive),
        (Screen::Receive, Screen::Tools),
        (Screen::Tools, Screen::Node),
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
    for screen in Screen::ALL {
        let text = sim_text(&sim(model(&w.ctx, screen)), 120, 40);
        assert!(!text.trim().is_empty(), "{screen:?} rendered blank");
        assert!(text.contains('●'), "{screen:?} lost its badge:\n{text}");
        match screen {
            // The node fixture's height, so the hero is reading the mock and not a default —
            // grouped, because every block number on every surface is (W7).
            Screen::Node => assert!(text.contains("3 541 122"), "{text}"),
            // The wallet fixture is open, so a balance and a subaddress table are there.
            Screen::Wallet => assert!(text.contains("XMR"), "{text}"),
            Screen::Send => {
                assert!(text.contains("Fee level"), "{text}");
                assert!(text.contains("normal"), "{text}");
                assert!(text.contains("Fee ceiling"), "{text}");
            }
            Screen::Log => assert!(text.contains("synced"), "{text}"),
            Screen::Receive => assert!(text.contains("Addresses"), "{text}"),
            Screen::Tools => {
                for group in ["Wallet", "Care", "Offline", "Proofs"] {
                    assert!(text.contains(group), "{text}");
                }
            }
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
fn the_send_screen_prepares_all_but_never_relays_without_review() {
    let w = World::new("sweep-review");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    type_str(&mut sim, DEST);
    sim.inject_event(key(KeyCode::Down));
    type_str(&mut sim, "all");
    sim.inject_event(key(KeyCode::Enter));
    run_action(&mut sim);
    assert!(matches!(&sim.model().send.stage, Stage::Built(plan) if plan.is_sweep));
    assert_eq!(w.count("sweep_all"), 1);
    assert_eq!(w.count("relay_tx"), 0);
    type_str(&mut sim, CONFIRM_WORD);
    sim.inject_event(key(KeyCode::Enter));
    assert!(
        sim.model().action.is_none(),
        "unrendered review cannot be submitted"
    );
    assert_eq!(w.count("relay_tx"), 0);
    sim.inject_event(key(KeyCode::Escape));
    assert!(matches!(sim.model().send.stage, Stage::Form));
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

    let _ = sim_text(&sim, 120, 40);
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
    sim.inject_event(paste(DEST));
    // v5.5: fields move with `↑↓`. `Tab` switches screens now, on every screen and in every
    // focus state (§7a.1, and §5b.4 row 10 records the binding this takes away).
    sim.inject_event(key(KeyCode::Down));
    sim.inject_event(paste("0.1"));
    assert_eq!(w.count("transfer_split"), 0, "the form must send nothing");

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
    assert_eq!(plan.txs.len(), 1, "this fixture prepares one transaction");
    assert_eq!(plan.txs[0].txid, BUILT_TXID);
    assert_eq!(
        w.count("transfer_split"),
        1,
        "exactly one split preparation"
    );
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
        shown.contains("Type send"),
        "the modal must demand the word: {shown}"
    );

    // §7a's resize table: the confirm modal is never abbreviated, so the narrow tier still
    // shows the recipient, the built fee and the word it wants.
    let mut narrow = String::new();
    for _ in 0..24 {
        narrow.push_str(&sim_text(&sim, 50, 16));
        sim.inject_event(key(KeyCode::Down));
    }
    for needle in ["Amount", "Fee", "Total", "Type send"] {
        assert!(
            narrow.contains(needle),
            "the scrollable narrow review lost {needle}:\n{narrow}"
        );
    }

    sim.inject_event(paste("send\n"));
    assert!(
        sim.model().send.typed.is_empty(),
        "paste must not confirm a payment"
    );
    assert_eq!(w.count("relay_tx"), 0);

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
    let _ = sim_text(&sim, 120, 40);
    type_str(&mut sim, CONFIRM_WORD);
    sim.inject_event(key(KeyCode::Enter));

    // 5. Relaying → the SAME plan (§2a.3).
    run_action(&mut sim);
    assert_eq!(w.count("transfer_split"), 1, "the plan must NOT be rebuilt");
    assert_eq!(w.count("relay_tx"), 1, "exactly one relay_tx");
    let relayed = w.body("relay_tx")["hex"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let built = std::fs::read_to_string(fixtures().join("wallet/transfer_split-do-not-relay.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["tx_metadata_list"][0].as_str().map(str::to_string))
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
    let mut ctx = test_context(&dir).expect("test context");
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
    for screen in Screen::ALL {
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
                    flowed_frame.contains("(P) Clear that record")
                        && find(s.model(), cw, ch, |t| t
                            == Target::Button(Button::ClearPending))
                        .is_some(),
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
    assert!(shown.contains("Type send"), "{shown}");
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
    let mut ctx = test_context(&dir).expect("test context");
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
    assert!(text.contains("(p) Check this node"), "{text}");
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
fn tab_switches_screens_only_when_no_editor_owns_focus() {
    for from in Screen::ALL {
        let mut m = offline_model(from);
        m.send.editing = false;
        let _ = m.on_key(KeyEvent::new(KeyCode::Tab));
        assert_eq!(m.screen, Screen::from_index(from.index() + 1));
    }
    for screen in [Screen::Node, Screen::Wallet, Screen::Log] {
        let mut m = offline_model(screen);
        let _ = m.on_key(KeyEvent::new(KeyCode::Char('/')));
        for ch in "q123456r?".chars() {
            let _ = m.on_key(KeyEvent::new(KeyCode::Char(ch)));
        }
        let _ = m.on_key(KeyEvent::new(KeyCode::Tab));
        assert_eq!(m.screen, screen, "focused filter owns mnemonics and Tab");
        let text = match screen {
            Screen::Node => m.picker.filter.value(),
            Screen::Log => m.logs.search.value(),
            _ => m.wallet.filter.value(),
        };
        assert_eq!(text, "q123456r?");
    }
}

#[test]
fn shift_tab_goes_back_from_every_unfocused_screen() {
    for from in Screen::ALL {
        let mut m = offline_model(from);
        m.send.editing = false;
        let _ = m.on_key(KeyEvent::new(KeyCode::BackTab));
        assert_eq!(
            m.screen,
            Screen::from_index(from.index() + Screen::ALL.len() - 1)
        );
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
    assert!(matches!(&s.model().modal, Some(Modal::CreateWallet(form)) if form.field == 1));
    // And it is documented IN the modal — on its own line under the controls, which is a
    // rendering assertion and not a status-bar one: a one-row footer clipped that line away, so
    // the rule was true only in DESIGN.md.
    let text = sim_text(&s, 110, 34);
    // M23: `first` no longer dangles — what Tab normally does is change screens.
    assert!(flowed(&text).contains("Tab moves fields"), "{text}");
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
    assert_eq!(s.model().screen, Screen::Wallet);
    assert!(s.model().wallet.filter_open, "Tab stays within the editor");
    assert_eq!(
        s.model().wallet.filter.value(),
        "abc",
        "Tab commits the filter — Esc is what discards"
    );
    s.inject_event(key(KeyCode::Enter));
    s.inject_event(key(KeyCode::Tab));
    assert_eq!(s.model().screen, Screen::Send);
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
fn the_priority_and_account_selectors_have_mouse_keyboard_parity() {
    let w = World::new("send-selectors");
    for (width, height) in [(40, 10), (80, 24), (120, 40)] {
        let mut mouse = sim(model(&w.ctx, Screen::Send));
        let mut keyboard = sim(model(&w.ctx, Screen::Send));
        for (field, selector) in [
            (send::Field::Priority, super::hit::Selector::Priority),
            (send::Field::Account, super::hit::Selector::Account),
        ] {
            mouse.model_mut().send.field = field;
            keyboard.model_mut().send.field = field;
            for delta in [1, 1, -1] {
                let at = find(mouse.model(), width, height, |t| {
                    t == Target::Arrow {
                        which: selector,
                        delta,
                    }
                })
                .expect("visible selector arrow");
                mouse.inject_event(click(at.0, at.1));
                keyboard.inject_event(key(if delta > 0 {
                    KeyCode::Right
                } else {
                    KeyCode::Left
                }));
                assert_eq!(
                    mouse.model().send.priority,
                    keyboard.model().send.priority,
                    "priority at {width}x{height}"
                );
                assert_eq!(
                    mouse.model().send.account,
                    keyboard.model().send.account,
                    "account at {width}x{height}"
                );
            }
        }
    }
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
        (Button::CreateWallet, "Create encrypted wallet"),
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
                "╭╮╰╯│─├┤┌┐└┘".contains(first) || {
                    let x = line.chars().position(|c| !c.is_whitespace()).unwrap() as u16;
                    matches!(
                        s.model().hits.at(x, y as u16, None),
                        Some(Target::Button(_))
                    )
                },
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
        for want in ["(O)pen a wallet", "(C)reate a wallet"] {
            assert!(text.contains(want), "{want} missing at {w}x{h}:\n{text}");
        }
        assert!(
            !text.contains("(W) Start the wallet service"),
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
            "(Enter) Confirm",
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
            .position(|l| l.contains("Keys · ↑↓ scroll · Esc closes"))
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
            text.contains("(Enter) Add"),
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

/// Nothing is selected on entry. The current target remains selectable for a confirmed retry.
#[test]
fn the_picker_selects_nothing_on_entry_and_can_retry_the_configured_node() {
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
    // The configured node can be retried after an uncertain connection update.
    s.inject_event(key(KeyCode::Down));
    assert_eq!(s.model().picker.table.borrow().selected, Some(0));
    s.inject_event(key(KeyCode::Enter));
    assert!(matches!(
        s.model().modal,
        Some(super::Modal::UseNode { .. })
    ));
    s.inject_event(key(KeyCode::Escape));
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
    assert!(text.contains("(f) Get the list"), "{text}");
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
fn new_wallet_backup_guidance_has_an_actual_recovery_action_and_no_secret_display() {
    let w = World::new("backup-guidance");
    let mut m = model(&w.ctx, Screen::Wallet);
    let _ = m.update(Msg::ActionDone(Box::new(Ok(subs::Outcome::Created {
        name: "fresh".into(),
    }))));
    let text = flowed(&modal_text(&m, 120, 40));
    assert!(text.contains("encrypted recovery wallet"), "{text}");
    assert!(
        text.contains("Labels, contacts, notes and history metadata are not included"),
        "{text}"
    );
    assert!(
        text.contains("Recovery words and private keys never appear on screen"),
        "{text}"
    );
    for (width, height) in [(40, 10), (80, 24), (120, 40)] {
        assert!(find(&m, width, height, |t| t
            == Target::Button(Button::Tool(
                super::advanced::Tool::RecoveryBackup
            )))
        .is_some());
        assert!(find(&m, width, height, |t| t == Target::ModalButton(1)).is_some());
    }
    let _ = m.on_key(KeyEvent::new(KeyCode::Escape));
    assert!(m.modal.is_none());
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
        let narrow = w < 76;
        let titles: [&str; 6] = if narrow {
            ["1", "2", "3", "4", "5", "6"]
        } else {
            [
                "1 Node",
                "2 Wallet",
                "3 Send",
                "4 Logs",
                "5 Receive",
                "6 Tools",
            ]
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
            let first = if narrow { nat } else { nat - 1 };
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
    let mut ctx = test_context(&dir).expect("test context");
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
fn the_keys_table_describes_scoped_field_navigation() {
    let navigation = KEYS
        .iter()
        .find(|(keys, _)| keys.contains("1–6"))
        .expect("six screen aliases");
    assert!(navigation.1.contains("Fields and dialogs keep Tab"));
    let mut m = offline_model(Screen::Node);
    m.show_help = true;
    let text = screen_text(&m, 110, 44);
    assert!(!text.contains("always, even inside a field"));

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
    // Tab belongs to the editor; Escape explicitly returns navigation to the screen.
    let mut s2 = sim(offline_model(Screen::Send));
    type_str(&mut s2, "9");
    s2.inject_event(key(KeyCode::Tab));
    assert_eq!(s2.model().screen, Screen::Send);
    assert_eq!(s2.model().send.field, send::Field::Amount);
    assert_eq!(s2.model().send.to.value(), "9");
    s2.inject_event(key(KeyCode::Escape));
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
                password: fmonero_core::SecretString::new("x".into()).unwrap(),
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
        let mut ctx = test_context(&dir).expect("test context");
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
    let mut ctx = test_context(&dir).expect("test context");
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

/// A config read failure preserves the last valid settings internally but masks public data
/// and invalidates review permission until a fresh valid policy is available. Warn once.
#[test]
fn an_unreadable_settings_file_keeps_values_but_masks_data_and_warns_once() {
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
    assert!(
        !text.contains(NODE_A),
        "failed policy must hide node data: {text}"
    );
    assert!(text.contains("Privacy on"), "{text}");
    assert!(!s.model().config_valid);
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
    let mut ctx = test_context(&dir).expect("test context");
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
    s.inject_event(key(KeyCode::Char('I')));
    let text = sim_text(&s, 150, 40);
    assert!(
        text.contains("monero-wallet-rpc.log"),
        "the source details name the selected file:\n{text}"
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
    std::fs::write(
        w.dir.join("systemctl.txt"),
        "fmonero-monerod.service inactive disabled yes 0\n",
    )
    .unwrap();
    for (k, want) in [
        (
            'u',
            vec![
                "Start your node",
                "fmonero starts your node on this machine.",
                "keeps its copy of the blockchain in",
                "120 GB",
                "the first sync takes hours",
                "(Enter) Confirm",
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
                "(Enter) Confirm",
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
        !text.contains("(W) Start the wallet service") && !text.contains("(W) Stop the wallet"),
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
        text.contains("(W) Stop the wallet service")
            && !text.contains("(W) Start the wallet service"),
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
    let mut ctx = test_context(&dir).expect("test context");
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
    let mut ctx = test_context(&dir).expect("test context");
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
    let mut ctx = test_context(&dir).expect("test context");
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
            "(Enter) Confirm (Esc) Cancel",
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
        text.contains("(P) Clear that record"),
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
        "(Enter) Confirm (Esc) Cancel",
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
            flowed(&text.replace('│', " ")).contains("(Esc) Cancel"),
            "{cw}x{ch}: the modal said nothing about Tab:\n{text}"
        );
        // The confirm word's own line is still on screen: the notice is a row the modal grew, not a
        // row it took (§7a: the safety surface is never abbreviated).
        assert!(
            flowed(&text.replace('│', " ")).contains("Type send to confirm"),
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
    w.open_fixture_wallet();
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
    s.inject_event(key(KeyCode::Char('e')));
    run_action(&mut s);
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
fn log_source_info_keeps_long_paths_complete_and_switching_is_explicit() {
    let mut m = offline_model(Screen::Log);
    let path = m.ctx.paths.run_dir.join(format!(
        "{}/monero-wallet-rpc.log",
        "long-path-segment".repeat(5)
    ));
    m.ctx.paths.run_dir = path.parent().unwrap().to_path_buf();
    assert_eq!(m.logs.source, fmonero_core::log_sources::LogSource::Wallet);
    let _ = m.on_log_key(KeyEvent::new(KeyCode::Char('I')));
    for (width, height) in [(80, 24), (120, 40)] {
        let text = modal_text(&m, width, height);
        let flat = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        assert!(flat.contains(&path.display().to_string()), "{text}");
    }
    let _ = m.on_modal_key(KeyEvent::new(KeyCode::Escape));
    let _ = m.on_log_key(KeyEvent::new(KeyCode::Char('S')));
    assert_eq!(m.logs.source, fmonero_core::log_sources::LogSource::Node);
    assert!(
        m.logs.empty,
        "a new source must not show the old source's lines"
    );
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
        assert!(!text.contains("(O)pen a wallet"), "{w}x{h}:\n{text}");
        assert!(text.contains("(C)reate a wallet"), "{w}x{h}:\n{text}");
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
    assert!(text.contains("(O)pen a wallet"), "{text}");
}

#[test]
fn wallet_badges_distinguish_closed_syncing_and_busy() {
    let world = World::new("wallet-badges");
    let mut status = fmonero_core::status::status(&world.ctx).unwrap();
    let mut m = model(&world.ctx, Screen::Wallet);
    status.wallet.open = false;
    status.wallet.reachable = true;
    m.status = Some(status.clone());
    assert_eq!(m.wallet_badge(), "closed");
    status.wallet.open = true;
    status.wallet.height = Some(0);
    m.status = Some(status.clone());
    assert_eq!(m.wallet_badge(), "syncing");
    assert!(super::screens::wallet_sync_line(&status)
        .unwrap()
        .starts_with("Syncing 0%"));
    status.wallet.busy = true;
    status.wallet.reachable = false;
    status.wallet.open = false;
    m.status = Some(status);
    assert_eq!(m.wallet_badge(), "busy");
    let text = screen_text(&m, 120, 40);
    assert!(text.contains("Waiting for the wallet"), "{text}");
    assert!(!text.contains("No wallet is open"), "{text}");
}

#[test]
fn status_updates_advance_the_wallet_confirmation_clock() {
    let world = World::new("confirmation-clock");
    let mut status = fmonero_core::status::status(&world.ctx).unwrap();
    let mut m = model(&world.ctx, Screen::Wallet);
    m.wallet.wh = Some(1);
    status.wallet.height = Some(2345);
    let _ = m.update(Msg::Status(Box::new(Ok(status))));
    assert_eq!(m.wallet.wh, Some(2345));
    assert!(m.wallet.now.is_some());
}

#[test]
fn tui_wallet_open_uses_the_refresh_budget_for_followup_reads() {
    let mut world = World::with_wallet("open-slow-reads", &["--delay-ms", "80"]);
    world.ctx.timeouts.wallet = std::time::Duration::from_millis(20);
    world.ctx.timeouts.refresh = std::time::Duration::from_secs(1);
    let announce = Arc::new(std::sync::Mutex::new(Vec::new()));
    let result = subs::execute(
        &world.ctx,
        subs::Action::WalletOpen {
            name: "main".into(),
            password: fmonero_core::SecretString::empty(),
        },
        &announce,
        &CancelToken::new(),
    );
    assert!(matches!(result, Ok(subs::Outcome::Opened(_))));
}

#[test]
fn invalid_settings_pause_every_poll_until_the_file_is_fixed() {
    let world = World::new("invalid-settings-pause");
    world.ctx.settings.save(&world.ctx).unwrap();
    let mut m = model(&world.ctx, Screen::Wallet);
    std::fs::write(world.ctx.paths.config_file(), "invalid = = =").unwrap();
    let _ = m.update(Msg::Poll);
    assert!(!m.config_valid);
    assert!(!m.poll_inflight);
    m.note.clear();
    let _ = m.update(Msg::Poll);
    assert!(!m.poll_inflight);
    assert!(m.note.is_empty());
    world.ctx.settings.save(&world.ctx).unwrap();
    let _ = m.update(Msg::Poll);
    assert!(m.config_valid, "{}", m.note);
    assert!(m.poll_inflight);
}

#[test]
fn a_wallet_change_discards_old_addresses_and_late_responses() {
    let world = World::new("wallet-data-generation");
    let mut status = fmonero_core::status::status(&world.ctx).unwrap();
    status.wallet.name = Some("first".into());
    let mut m = model(&world.ctx, Screen::Wallet);
    m.status = Some(status.clone());
    m.wallet.addresses = Some(Default::default());
    let old_epoch = m.read_epoch;
    status.wallet.name = Some("second".into());
    let _ = m.update(Msg::Status(Box::new(Ok(status))));
    assert!(m.wallet.addresses.is_none());
    assert_ne!(m.read_epoch, old_epoch);
    let _ = m.update(Msg::WalletData(
        old_epoch,
        Box::new(Ok((Default::default(), Default::default()))),
    ));
    assert!(m.wallet.addresses.is_none());
    assert!(
        m.wallet.inflight,
        "an old result must not clear the new request's guard"
    );
    let mut stale = m.status.clone().unwrap();
    stale.wallet.name = Some("first".into());
    let _ = m.update(Msg::StatusRead(old_epoch, Box::new(Ok(stale))));
    assert_eq!(
        m.status.as_ref().unwrap().wallet.name.as_deref(),
        Some("second")
    );
}

#[test]
fn late_history_does_not_rewind_confirmations_or_replace_a_newer_request() {
    let world = World::new("history-generation");
    let status = fmonero_core::status::status(&world.ctx).unwrap();
    let mut m = model(&world.ctx, Screen::Wallet);
    let _ = m.load_history();
    let seq = m.history_seq;
    let revision = m.status_revision;
    let _ = m.update(Msg::Status(Box::new(Ok(status.clone()))));
    let _ = m.update(Msg::History(
        m.read_epoch,
        seq,
        revision,
        Box::new(Ok((vec![], Some(1), Some(1)))),
    ));
    assert_eq!(m.wallet.wh, status.wallet.height);
    let _ = m.load_history();
    let _ = m.update(Msg::History(
        m.read_epoch,
        seq,
        revision,
        Box::new(Err("obsolete failure".into())),
    ));
    assert!(m.wallet.rows_err.is_none());
}

fn paste(text: &str) -> Event {
    Event::Paste(ftui::PasteEvent::bracketed(text))
}

#[test]
fn paste_edits_only_the_active_send_text_field_and_never_submits() {
    let mut m = offline_model(Screen::Send);
    let _ = m.update(Msg::Raw(paste(&format!("{DEST}\n"))));
    assert_eq!(m.send.to.value().trim(), DEST);
    assert_eq!(m.screen, Screen::Send);
    assert!(matches!(m.send.stage, Stage::Form));
    assert!(m.action.is_none());
    m.send.focus(send::Field::Amount);
    let _ = m.update(Msg::Raw(paste("0.1")));
    assert_eq!(m.send.request(None).unwrap().amount.0, 100_000_000_000);
    m.send.focus(send::Field::Note);
    let _ = m.update(Msg::Raw(paste("q\nsend\t3")));
    assert_eq!(m.send.note.value(), "q send 3");
    assert!(m.action.is_none());
    for field in [send::Field::Priority, send::Field::Account] {
        m.send.focus(field);
        let _ = m.update(Msg::Raw(paste("9")));
        assert_eq!(m.send.to.value().trim(), DEST);
    }
    m.send.focus(send::Field::To);
    m.send.editing = false;
    let _ = m.update(Msg::Raw(paste("ignored")));
    m.send.editing = true;
    m.send.stage = Stage::Validating;
    let _ = m.update(Msg::Raw(paste("ignored")));
    m.send.stage = Stage::Form;
    m.show_help = true;
    let _ = m.update(Msg::Raw(paste("ignored")));
    m.show_help = false;
    m.send.picker = Some((vec![], 0));
    let _ = m.update(Msg::Raw(paste("ignored")));
    assert_eq!(m.send.to.value().trim(), DEST);
}

#[test]
fn paste_reaches_visible_modal_and_filter_inputs_without_leaking_to_send() {
    let mut m = offline_model(Screen::Send);
    m.modal = Some(Modal::AddNode {
        input: text_input(&m.theme),
    });
    let _ = m.update(Msg::Raw(paste("node.example:18081\n")));
    let Some(Modal::AddNode { input }) = &m.modal else {
        panic!("modal dismissed");
    };
    assert_eq!(input.value().trim(), "node.example:18081");
    assert!(m.send.to.value().is_empty());
    m.modal = Some(Modal::OpenWallet {
        names: vec![],
        sel: 0,
        pass: super::editor::SecretInput::new(),
        asking: false,
    });
    let _ = m.update(Msg::Raw(paste("sample")));
    if let Some(Modal::OpenWallet { pass, asking, .. }) = &mut m.modal {
        assert!(pass.value().is_empty());
        *asking = true;
        pass.set_focused(true);
    }
    let _ = m.update(Msg::Raw(paste("sample")));
    let Some(Modal::OpenWallet { pass, .. }) = &m.modal else {
        panic!("modal dismissed");
    };
    assert_eq!(pass.value().len(), 6);
    m.modal = None;
    m.screen = Screen::Wallet;
    m.wallet.filter_open = true;
    let _ = m.update(Msg::Raw(paste("coffee")));
    assert_eq!(m.wallet.filter.value(), "coffee");
    m.screen = Screen::Node;
    m.picker.filter_open = true;
    let _ = m.update(Msg::Raw(paste("example")));
    assert_eq!(m.picker.filter.value(), "example");
    assert!(m.send.to.value().is_empty());
    assert!(m.action.is_none());
}

#[test]
fn privacy_hides_public_sensitive_data_and_errors_on_every_screen() {
    for screen in Screen::ALL {
        let mut m = offline_model(screen);
        m.ctx.settings.tui.privacy = true;
        m.note = "private-marker-address-amount-error".into();
        m.status_err = Some(m.note.clone());
        m.wallet.err = Some(m.note.clone());
        m.wallets = vec![m.note.clone()];
        m.logs.viewer.borrow_mut().push(m.note.clone());
        m.send.to.set_value(&m.note);
        m.send.amount.set_value(&m.note);
        m.send.note.set_value(&m.note);
        for (width, height) in [(40, 10), (60, 20), (120, 40)] {
            let text = screen_text(&m, width, height);
            assert!(
                !text.contains("private-marker"),
                "{screen:?} {width}x{height}: {text}"
            );
            assert!(text.contains("Privacy"));
        }
    }
}

#[test]
fn restore_form_owns_mnemonics_and_secrets_at_every_supported_size() {
    let mut m = offline_model(Screen::Tools);
    let _ = m.open_restore();
    let _ = m.on_key(KeyEvent::new(KeyCode::Tab));
    for ch in "q123456r?secret".chars() {
        let _ = m.on_key(KeyEvent::new(KeyCode::Char(ch)));
    }
    assert_eq!(m.screen, Screen::Tools);
    for (width, height) in [(40, 10), (60, 20), (120, 40)] {
        let text = screen_text(&m, width, height);
        assert!(!text.contains("secret"), "{text}");
        assert!(find(&m, width, height, |t| t == Target::Field(1)).is_some());
        assert!(find(&m, width, height, |t| t == Target::ModalButton(1)).is_some());
    }
    let _ = m.on_key(KeyEvent::new(KeyCode::Escape));
    assert!(m.modal.is_none());
    assert!(m.action.is_none());
}

#[test]
fn six_screen_tabs_have_exact_click_regions_after_resize() {
    for (width, height) in [(40, 10), (60, 20), (120, 40)] {
        let m = offline_model(Screen::Wallet);
        for index in 0..6 {
            assert!(
                find(&m, width, height, |t| t == Target::Tab(index)).is_some(),
                "missing tab {index} at {width}x{height}"
            );
        }
    }
}

#[test]
fn stale_worker_completion_cannot_replace_current_action() {
    let mut m = offline_model(Screen::Node);
    let _ = m.start(subs::Action::NodeUp);
    let id = m.action.as_ref().unwrap().id;
    let _ = m.update(Msg::ActionFinished(
        id + 1,
        Box::new(Ok(subs::Outcome::Note("stale-result".into()))),
    ));
    assert_eq!(m.action.as_ref().unwrap().id, id);
    assert!(!m.note.contains("stale-result"));
    let _ = m.cancel_action();
    assert!(m.action.as_ref().unwrap().cancelling);
    assert!(
        m.action.is_some(),
        "wait for outcome, do not enable a concurrent action"
    );
}

#[test]
fn privacy_setting_does_not_change_when_review_reveal_is_requested() {
    let w = World::new("private-review");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    sim.inject_event(paste(DEST));
    sim.inject_event(key(KeyCode::Down));
    sim.inject_event(paste("0.1"));
    sim.inject_event(key(KeyCode::Enter));
    run_action(&mut sim);
    assert!(matches!(sim.model().send.stage, Stage::Built(_)));
    sim.model_mut().ctx.settings.tui.privacy = true;
    let hidden = sim_text(&sim, 120, 40);
    assert!(!hidden.contains(&DEST[..30]));
    sim.inject_event(key(KeyCode::Char('V')));
    assert!(sim.model().ctx.settings.tui.privacy);
    assert!(sim.model().send.review_revealed);
    let revealed = sim_text(&sim, 120, 40);
    assert!(revealed.contains(&DEST[..30]));
    assert!(revealed.contains(&DEST[DEST.len() - 16..]));
    sim.inject_event(key(KeyCode::Escape));
    assert!(!sim.model().send.review_revealed);
    assert!(sim.model().ctx.settings.tui.privacy);
    assert_eq!(w.count("relay_tx"), 0);
}

#[test]
fn every_tools_action_remains_mouse_reachable_in_short_terminals() {
    for (width, height) in [(40, 10), (60, 16), (120, 40)] {
        let mut m = offline_model(Screen::Tools);
        for group in 0..4 {
            let _ = m.click_button(Button::ToolGroup(group));
            let items = super::workflows::tool_items(group);
            for (index, (action, _, _)) in items.iter().enumerate() {
                m.tools_selected = index;
                assert!(
                    find(&m, width, height, |target| target
                        == Target::Button(*action))
                    .is_some(),
                    "Tools action {action:?} unreachable at {width}x{height}"
                );
                assert!(find(&m, width, height, |target| target
                    == Target::Button(Button::ToolsDown))
                .is_some());
            }
        }
    }
}

#[test]
fn every_protected_tool_field_stays_masked_with_cancel_visible_after_resize() {
    use super::workflows::{Form, Value};
    let tools = [
        super::advanced::Tool::ChangePassword,
        super::advanced::Tool::CreateViewOnly,
        super::advanced::Tool::RecoveryBackup,
        super::advanced::Tool::CreateHardware,
        super::advanced::Tool::SignUnsigned,
        super::advanced::Tool::SubmitSigned,
    ];
    for tool in tools {
        for privacy in [false, true] {
            let mut m = offline_model(Screen::Tools);
            m.ctx.settings.tui.privacy = privacy;
            let mut form = Form::tool(&m, tool, None);
            let secret_positions = form
                .fields
                .iter()
                .enumerate()
                .filter_map(|(i, f)| matches!(f.value, Value::Secret(_)).then_some(i))
                .collect::<Vec<_>>();
            for index in secret_positions {
                form.focus(index);
                form.event(&paste("NEVER_RENDER_PLAINTEXT_8372"));
            }
            m.modal = Some(Modal::Workflow(form));
            for (width, height) in [(40, 10), (60, 16), (120, 40)] {
                let text = screen_text(&m, width, height);
                assert!(!text.contains("NEVER_RENDER"), "{tool:?}: {text}");
                assert!(
                    find(&m, width, height, |target| target == Target::ModalButton(1)).is_some()
                );
            }
            let _ = m.on_key(KeyEvent::new(KeyCode::Escape));
            assert!(m.modal.is_none());
            assert!(
                m.action.is_none(),
                "cancelling a form must not contact a backend"
            );
        }
    }
}

#[test]
fn native_submit_form_only_starts_review_and_moves_its_password() {
    use super::{
        advanced::Tool,
        workflows::{Form, Value},
    };
    let mut m = offline_model(Screen::Tools);
    let identity = fmonero_core::wallet_ops::WalletIdentity {
        name: "isolated-test".into(),
        network: fmonero_core::NetType::Mainnet,
        primary_address: DEST.into(),
    };
    let mut form = Form::tool(&m, Tool::SubmitSigned, Some(identity.clone()));
    for (i, value) in [
        (0, "/tmp/explicit-signed-artifact"),
        (1, "protected-test-password"),
        (2, "0.000000000001"),
    ] {
        form.focus(i);
        match &mut form.fields[i].value {
            Value::Public(input) => input.set_value(value),
            Value::Secret(input) => {
                input.handle_event(&paste(value));
            }
        }
    }
    form.focus(2);
    m.modal = Some(Modal::Workflow(form));
    let _ = m.on_key(KeyEvent::new(KeyCode::Enter));
    assert!(m.action.is_none(), "first Enter reviews the form");
    let _ = m.on_key(KeyEvent::new(KeyCode::Char('q')));
    let _ = m.on_key(KeyEvent::new(KeyCode::Enter));
    let slot = m
        .action
        .as_ref()
        .expect("review startup is armed")
        .slot
        .clone();
    let action = slot.lock().unwrap().take().unwrap();
    assert!(!format!("{action:?}").contains("protected-test-password"));
    let subs::Action::Native(job) = action else {
        panic!("the form must only begin review, never submit");
    };
    let super::native::Job::Begin {
        wallet,
        password,
        source,
        destination,
        max_fee,
        signed,
    } = *job
    else {
        panic!("the form must only begin review, never submit");
    };
    assert_eq!(wallet, identity);
    assert_eq!(password.expose_secret(), "protected-test-password");
    assert_eq!(source, PathBuf::from("/tmp/explicit-signed-artifact"));
    assert!(destination.is_none());
    assert!(signed);
    assert_eq!(max_fee, Some(Piconero(1)));
    assert!(!m.ctx.settings.tui.privacy);
}

#[test]
fn too_small_terminal_cannot_submit_an_invisible_form() {
    let mut m = offline_model(Screen::Tools);
    let _ = m.open_restore();
    let _ = screen_text(&m, 30, 8);
    let _ = m.on_key(KeyEvent::new(KeyCode::Enter));
    assert!(m.action.is_none());
    let Some(Modal::Workflow(form)) = &m.modal else {
        panic!("form stays open");
    };
    assert_eq!(form.focus, 0, "invisible controls cannot advance");
    let _ = m.on_key(KeyEvent::new(KeyCode::Escape));
    assert!(m.modal.is_none(), "Escape remains available at tiny sizes");
}

#[test]
fn wheel_over_a_modal_button_never_activates_it() {
    let m = offline_model(Screen::Tools);
    let area = ftui::layout::Rect::new(3, 3, 20, 10);
    m.hits.modal(area);
    m.hits
        .button(Button::NativeReveal, ftui::layout::Rect::new(5, 5, 10, 1));
    assert_eq!(
        m.hits.at(5, 5, None),
        Some(Target::Button(Button::NativeReveal))
    );
    assert_eq!(m.hits.at(5, 5, Some(false)), None);
}

#[test]
fn privacy_blocks_copy_before_starting_any_clipboard_process() {
    let mut m = offline_model(Screen::Wallet);
    m.ctx.settings.tui.privacy = true;
    m.copy("sensitive-public-identifier");
    assert!(m.note.contains("Privacy hides this value"));
    assert!(!m.note.contains("sensitive-public-identifier"));
}

#[test]
fn log_snapshots_are_sanitized_before_display_and_search() {
    let mut m = offline_model(Screen::Log);
    let lines = vec![
        "ordinary log text".into(),
        "password: must-not-render".into(),
        "\x1b]52;c;CLIPBOARD_PAYLOAD\x07safe visible text".into(),
        "\x1b]8;;https://private-target.invalid\x1b\\public label\x1b]8;;\x1b\\".into(),
    ];
    let _ = m.update(Msg::LogLines(Box::new(Ok((
        lines,
        PathBuf::from("/tmp/public-test-log"),
    )))));
    let text = screen_text(&m, 120, 40);
    for forbidden in [
        "must-not-render",
        "CLIPBOARD_PAYLOAD",
        "private-target.invalid",
    ] {
        assert!(!text.contains(forbidden), "{text}");
    }
    assert!(text.contains("ordinary log text"));
    assert!(text.contains("public label"));
    m.ctx.settings.tui.privacy = true;
    let hidden = screen_text(&m, 120, 40);
    assert!(!hidden.contains("ordinary log text"));
    assert!(hidden.contains("Source"));
}

#[test]
fn receive_details_never_infer_spending_authority_from_unlocked_outputs() {
    use fmonero_core::{
        receive::{Observation, ReceiveStage, ReceivingAddress},
        rpc::wallet::{AddressSet, SubaddressRow},
        wallet_ops::WalletIdentity,
    };
    let mut m = offline_model(Screen::Receive);
    m.wallet.addresses = Some(AddressSet {
        primary: DEST.into(),
        addresses: vec![SubaddressRow {
            address: DEST.into(),
            ..Default::default()
        }],
    });
    m.receiving.observed = Some(Observation {
        address: ReceivingAddress {
            wallet: WalletIdentity {
                name: "test-view-only".into(),
                network: fmonero_core::NetType::Mainnet,
                primary_address: DEST.into(),
            },
            account_index: 0,
            address_index: 0,
            address: DEST.into(),
            label: String::new(),
        },
        stage: ReceiveStage::Spendable,
        received: Piconero(1),
        confirmed: Piconero(1),
        spendable: Piconero(1),
        receipts: vec![],
        wallet_height: Some(10),
        pool_unavailable: true,
        spend_authority: None,
        spent_status_known: false,
    });
    let _ = m.receipt_details();
    let text = flowed(&modal_text(&m, 120, 40));
    assert!(text.contains("0.000000000001 XMR"), "{text}");
    assert!(text.contains("Spent status is unknown"), "{text}");
    assert!(
        text.contains("view-only wallet needs matching key images"),
        "{text}"
    );
    assert!(text.contains("no spend authority is implied"), "{text}");
    assert!(text.contains("Pool visibility is unavailable"), "{text}");
    m.ctx.settings.tui.privacy = true;
    let hidden = screen_text(&m, 120, 40);
    assert!(!hidden.contains(DEST));
    assert!(!hidden.contains("0.000000000001"));
}

#[test]
fn result_dialog_stops_at_last_row_and_can_scroll_back_immediately() {
    let mut m = offline_model(Screen::Tools);
    m.modal = Some(Modal::Result(super::advanced::ResultView::complete(
        "Public test result",
        (0..40).map(|i| format!("Detail row {i}")).collect(),
    )));
    for _ in 0..100 {
        let _ = screen_text(&m, 40, 10);
        let _ = m.click_button(Button::ResultDown);
    }
    let Some(Modal::Result(result)) = &m.modal else {
        panic!("result stays open");
    };
    assert_eq!(result.scroll, result.max_scroll.get());
    let last = result.scroll;
    let _ = m.click_button(Button::ResultUp);
    let Some(Modal::Result(result)) = &m.modal else {
        panic!("result stays open");
    };
    assert_eq!(result.scroll, last - 1);
    let _ = m.on_key(KeyEvent::new(KeyCode::Escape));
    assert!(m.modal.is_none());
}

#[test]
fn empty_wallet_chooser_has_clickable_create_restore_and_close_at_each_size() {
    for (width, height) in [(40, 10), (60, 16), (120, 40)] {
        for privacy in [false, true] {
            let mut m = offline_model(Screen::Wallet);
            m.ctx.settings.tui.privacy = privacy;
            m.wallets.clear();
            let _ = m.open_wallet_modal();
            assert!(find(&m, width, height, |t| t
                == Target::Button(Button::CreateWallet))
            .is_some());
            assert!(find(&m, width, height, |t| t == Target::Button(Button::Restore)).is_some());
            assert!(find(&m, width, height, |t| t == Target::ModalButton(1)).is_some());
            let _ = m.on_key(KeyEvent::new(KeyCode::Char('r')));
            assert!(matches!(m.modal, Some(Modal::Workflow(_))));
            assert!(m.action.is_none());
        }
    }
}

#[test]
fn tools_group_arrows_move_in_their_advertised_direction() {
    let mut m = offline_model(Screen::Tools);
    let _ = m.on_key(KeyEvent::new(KeyCode::Left));
    assert_eq!(m.tools_group, 3);
    let _ = m.on_key(KeyEvent::new(KeyCode::Right));
    assert_eq!(m.tools_group, 0);
}

#[test]
fn recipient_editor_keeps_contact_label_keys_without_opening_a_picker() {
    let mut m = offline_model(Screen::Send);
    for c in "@rentRNVq123".chars() {
        let _ = m.on_key(KeyEvent::new(KeyCode::Char(c)));
    }
    assert_eq!(m.send.to.value(), "@rentRNVq123");
    assert!(m.send.picker.is_none());
    assert_eq!(m.screen, Screen::Send);
    assert!(m.action.is_none());
}

#[test]
fn annotation_workers_use_reviewed_identity_and_refuse_changed_labels() {
    let w = World::new("annotation-identity");
    w.open_fixture_wallet();
    let client = fmonero_core::WalletClient::from_ctx(&w.ctx).unwrap();
    let row = client.get_address(0).unwrap().addresses[0].clone();
    let reply = super::annotations::Job::LabelForm { account: 0, row }
        .execute(&w.ctx)
        .unwrap();
    let subs::Outcome::AnnotationForm { what, .. } = reply else {
        panic!("expected editable annotation");
    };
    let result = super::annotations::Job::Save {
        what,
        value: "New receiving label".into(),
    }
    .execute(&w.ctx)
    .unwrap();
    assert!(matches!(result, subs::Outcome::Note(_)));
    assert_eq!(w.count("label_address"), 1);
    let params = w.body("label_address");
    assert_eq!(params["label"], "New receiving label");
    let mut stale = client.get_address(0).unwrap().addresses[0].clone();
    stale.label.push_str(" unexpected external edit");
    assert!(super::annotations::Job::LabelForm {
        account: 0,
        row: stale
    }
    .execute(&w.ctx)
    .is_err());
    assert_eq!(w.count("label_address"), 1, "stale form cannot mutate");
}

#[test]
fn payment_note_worker_preserves_the_observed_note_and_refuses_stale_review() {
    let w = World::new("note-identity");
    w.open_fixture_wallet();
    let client = fmonero_core::WalletClient::from_ctx(&w.ctx).unwrap();
    let row = client.get_transfer_by_txid(BUILT_TXID, 0).unwrap().unwrap();
    let reply = super::annotations::Job::NoteForm(row.clone())
        .execute(&w.ctx)
        .unwrap();
    let subs::Outcome::AnnotationForm { what, value, .. } = reply else {
        panic!("expected note form");
    };
    assert_eq!(value, row.note);
    let _ = super::annotations::Job::Save {
        what,
        value: "Reviewed note".into(),
    }
    .execute(&w.ctx)
    .unwrap();
    assert_eq!(w.count("set_tx_notes"), 1);
    assert_eq!(w.body("set_tx_notes")["notes"][0], "Reviewed note");
    let mut stale = row;
    stale.note.push_str(" stale");
    assert!(super::annotations::Job::NoteForm(stale)
        .execute(&w.ctx)
        .is_err());
    assert_eq!(w.count("set_tx_notes"), 1);
}

#[test]
fn external_privacy_changes_revoke_confirmation_without_dropping_the_plan() {
    let world = World::new("external-private-review");
    world.ctx.settings.save(&world.ctx).unwrap();
    let mut s = sim(model(&world.ctx, Screen::Send));
    s.inject_event(paste(DEST));
    s.inject_event(key(KeyCode::Down));
    s.inject_event(paste("0.1"));
    s.inject_event(key(KeyCode::Enter));
    run_action(&mut s);
    let _ = sim_text(&s, 120, 40);
    type_str(&mut s, "send");
    assert_eq!(s.model().send.typed, "send");
    assert!(s.model().send.review_visible.get());
    fmonero_core::settings::set_privacy(&world.ctx, true).unwrap();
    let before_poll = sim_text(&s, 120, 40);
    assert!(
        !before_poll.contains(&DEST[..30]),
        "metadata change masks before the next Poll"
    );
    assert!(!s.model().send.review_visible.get());
    assert!(s.model_mut().maybe_reload_settings());
    assert!(
        s.model().send.confirming(),
        "keep the immutable plan and its lock"
    );
    assert!(s.model().send.typed.is_empty());
    assert!(!s.model().send.review_visible.get());
    assert!(!s.model().send.review_revealed);
    let hidden = sim_text(&s, 120, 40);
    assert!(!hidden.contains(&DEST[..30]));
    s.inject_event(key(KeyCode::Char('V')));
    assert!(s.model().send.review_revealed);
    let _ = sim_text(&s, 120, 40);
    type_str(&mut s, "send");
    assert_eq!(s.model().send.typed, "send");
    fmonero_core::settings::set_privacy(&world.ctx, false).unwrap();
    assert!(s.model_mut().maybe_reload_settings());
    assert!(
        !s.model().privacy(),
        "only the explicit external setting turns privacy off"
    );
    assert!(s.model().send.typed.is_empty());
    assert!(!s.model().send.review_revealed);
    assert!(s.model().send.confirming());
    fmonero_core::settings::set_privacy(&world.ctx, true).unwrap();
    let before_poll = sim_text(&s, 120, 40);
    assert!(
        !before_poll.contains(&DEST[..30]),
        "metadata change masks before the next Poll"
    );
    assert!(!s.model().send.review_visible.get());
    assert!(s.model_mut().maybe_reload_settings());
    s.inject_event(key(KeyCode::Char('V')));
    assert!(s.model().send.review_revealed);
    std::fs::write(world.ctx.paths.config_file(), "invalid = = =").unwrap();
    assert!(!s.model_mut().maybe_reload_settings());
    s.inject_event(key(KeyCode::Char('V')));
    assert!(
        !s.model().send.review_revealed,
        "cannot reveal with unknown policy"
    );
    s.inject_event(key(KeyCode::Escape));
    assert!(!s.model().send.confirming());
    assert_eq!(world.count("relay_tx"), 0);
}

#[test]
fn payment_detail_controls_scroll_full_identifiers_and_block_background_clicks() {
    let mut m = offline_model(Screen::Wallet);
    m.wallet.rows = vec![fmonero_core::Transfer {
        txid: "1234567890abcdef".repeat(4),
        note: "Full note with Unicode 界".repeat(8),
        destinations: vec![fmonero_core::history::Destination {
            address: DEST.into(),
            amount: Piconero(1),
        }],
        ..Default::default()
    }];
    m.wallet.list.borrow_mut().selected = Some(0);
    m.wallet.detail = true;
    let mut read = String::new();
    for _ in 0..60 {
        read.push_str(&screen_text(&m, 40, 10));
        assert!(find(&m, 40, 10, |t| t == Target::Button(Button::DetailDown)).is_some());
        assert!(find(&m, 40, 10, |t| t == Target::Button(Button::DetailClose)).is_some());
        assert!(!matches!(m.hits.at(1, 0, None), Some(Target::Tab(_))));
        let _ = m.click_button(Button::DetailDown);
    }
    assert!(read.contains("1234567890abcdef"));
    assert!(read.contains(&DEST[DEST.len() - 16..]));
    let _ = m.click_button(Button::DetailClose);
    assert!(!m.wallet.detail);
}

#[test]
fn short_help_keeps_every_binding_reachable_with_clickable_navigation() {
    for (width, height) in [(40, 10), (70, 18), (120, 40)] {
        let mut m = offline_model(Screen::Wallet);
        let _ = m.on_key(KeyEvent::new(KeyCode::Char('?')));
        let mut all_rows = String::new();
        for _ in 0..200 {
            all_rows.push_str(&modal_text(&m, width, height));
            all_rows.push('\n');
            let at = find(&m, width, height, |t| t == Target::Button(Button::HelpDown))
                .expect("help More button");
            let _ = m.update(Msg::Raw(click(at.0, at.1)));
            assert!(m.show_help);
            assert_eq!(m.screen, Screen::Wallet);
        }
        let whole = all_rows
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        for (key, _) in KEYS {
            let expected = key
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>();
            assert!(
                whole.contains(&expected),
                "{width}x{height} lost binding {key}"
            );
        }
        assert_eq!(m.help_scroll, m.help_max.get());
        let back = find(&m, width, height, |t| t == Target::Button(Button::HelpUp)).unwrap();
        let _ = m.update(Msg::Raw(click(back.0, back.1)));
        assert_eq!(m.help_scroll, m.help_max.get().saturating_sub(1));
        let close = find(&m, width, height, |t| t == Target::Button(Button::Help)).unwrap();
        let _ = m.update(Msg::Raw(click(close.0, close.1)));
        assert!(!m.show_help);
    }
}

#[test]
fn wallet_mnemonics_and_clicks_open_the_same_workflows_without_shift() {
    for (key, button) in [
        ('o', Button::OpenWallet),
        ('c', Button::CreateWallet),
        ('r', Button::Restore),
    ] {
        let mut keyboard = offline_model(Screen::Wallet);
        let mut mouse = offline_model(Screen::Wallet);
        keyboard.wallets = vec!["example".into()];
        mouse.wallets = keyboard.wallets.clone();
        let _ = keyboard.on_key(KeyEvent::new(KeyCode::Char(key)));
        let at = find(&mouse, 80, 24, |t| t == Target::Button(button)).expect("mnemonic control");
        let _ = mouse.update(Msg::Raw(click(at.0, at.1)));
        assert_eq!(
            std::mem::discriminant(keyboard.modal.as_ref().unwrap()),
            std::mem::discriminant(mouse.modal.as_ref().unwrap())
        );
    }
    let mut closed = offline_model(Screen::Wallet);
    let _ = closed.on_key(KeyEvent::new(KeyCode::Char('n')));
    assert!(
        matches!(closed.modal, Some(Modal::OpenWallet { .. })),
        "New address on a closed wallet explains its prerequisite"
    );
}

#[test]
fn public_unicode_wrapping_preserves_tokens_and_terminal_width() {
    use unicode_width::UnicodeWidthStr;
    let text = "界界🙂café界end";
    for width in [3, 5, 9, 36] {
        let rows = super::screens::wrapped(text, width);
        assert_eq!(rows.concat(), text);
        assert!(rows.iter().all(|line| line.width() <= width));
        let words = super::screens::wrap_words(text, width);
        assert_eq!(words.concat(), text);
        assert!(words.iter().all(|line| line.width() <= width));
    }
}

#[test]
fn connection_confirmations_keep_scroll_and_cancel_clickable_at_small_sizes() {
    for (width, height) in [(40, 10), (70, 18), (120, 40)] {
        for privacy in [false, true] {
            for what in [
                super::Confirm::NodeUp,
                super::Confirm::NodeDown,
                super::Confirm::NodeLocal,
                super::Confirm::ClearPending,
            ] {
                let mut m = offline_model(Screen::Node);
                m.ctx.settings.tui.privacy = privacy;
                m.modal = Some(Modal::Confirm { what });
                let _ = modal_text(&m, width, height);
                assert!(find(&m, width, height, |t| t == Target::ModalButton(0)).is_some());
                let more = find(&m, width, height, |t| {
                    t == Target::Button(Button::DialogDown)
                })
                .expect("read more");
                let _ = m.update(Msg::Raw(click(more.0, more.1)));
                assert!(m.modal.is_some());
                assert!(m.action.is_none(), "scroll never starts an operation");
                let cancel = find(&m, width, height, |t| t == Target::ModalButton(1))
                    .expect("visible Cancel");
                let _ = m.update(Msg::Raw(click(cancel.0, cancel.1)));
                assert!(m.modal.is_none());
                assert!(m.action.is_none());
            }
        }
    }
}

#[test]
fn short_qr_dialog_keeps_the_full_address_and_close_action() {
    let mut m = offline_model(Screen::Receive);
    m.modal = Some(Modal::Qr {
        address: DEST.into(),
    });
    let text = modal_text(&m, 40, 10);
    let compact = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    assert!(compact.contains(DEST), "{text}");
    let close = find(&m, 40, 10, |t| t == Target::ModalButton(0)).unwrap();
    let _ = m.update(Msg::Raw(click(close.0, close.1)));
    assert!(m.modal.is_none());
}

#[test]
fn public_verification_forms_never_request_private_wallet_identity_or_password() {
    use super::advanced::Tool;
    for tool in [
        Tool::VerifyMessage,
        Tool::VerifyTxProof,
        Tool::VerifySpendProof,
        Tool::VerifyReserveProof,
    ] {
        let mut m = offline_model(Screen::Tools);
        let _ = m.open_tool(tool);
        assert!(
            m.action.is_none(),
            "opening a public form never contacts a service"
        );
        let Some(Modal::Workflow(form)) = &mut m.modal else {
            panic!("public form opens directly");
        };
        assert!(form.identity.is_none());
        assert!(form
            .fields
            .iter()
            .all(|f| matches!(f.value, super::workflows::Value::Public(_))));
        form.review = true;
        let mut text = String::new();
        for _ in 0..50 {
            text.push_str(&modal_text(&m, 80, 24));
            text.push('\n');
            let _ = m.on_modal_key(KeyEvent::new(KeyCode::Down));
        }
        let compact = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        assert!(compact.contains("Nowalletselectionorpassword"));
        assert!(m.action.is_none());
    }
}

#[test]
fn public_verification_dispatch_never_opens_or_identifies_a_wallet() {
    let w = World::new("public-verify-dispatch");
    // The fixture backend has no successful proof response. Exercise the production job
    // through its real check request and preserve that backend error instead of faking proof.
    let result = super::advanced::Job::VerifyMessage {
        address: DEST.into(),
        data: "public test message".into(),
        signature: "public test signature".into(),
    }
    .execute(&w.ctx, &CancelToken::new());
    assert!(result.is_err());
    assert_eq!(w.count("verify"), 1);
    assert_eq!(w.count("open_wallet"), 0);
    assert_eq!(w.count("get_address"), 0);
    assert_eq!(w.count("get_accounts"), 0);
}

fn native_pending_record() -> fmonero_core::native_tx::PendingSubmission {
    fmonero_core::native_tx::PendingSubmission {
        wallet: fmonero_core::wallet_ops::WalletIdentity {
            name: "main".into(),
            network: fmonero_core::NetType::Mainnet,
            primary_address: DEST.into(),
        },
        artifact_sha256: "c".repeat(64),
        transactions: 3,
        unknown_txids: 2,
        proven_txids: vec![BUILT_TXID.into()],
        attempted_at: 1234,
    }
}
fn write_native_pending(
    ctx: &Context,
    record: &fmonero_core::native_tx::PendingSubmission,
) -> Vec<u8> {
    ctx.paths.ensure_dirs().unwrap();
    let bytes = serde_json::to_vec(record).unwrap();
    fmonero_core::write_private(&ctx.paths.state_dir.join("native-pending.json"), &bytes).unwrap();
    bytes
}
fn assert_native_guard_unchanged(w: &World, bytes: &[u8]) {
    assert_eq!(
        std::fs::read(w.ctx.paths.state_dir.join("native-pending.json")).unwrap(),
        bytes
    );
    assert!(fmonero_core::send::refuse_if_send_in_flight(&w.ctx).is_err());
    for method in [
        "transfer",
        "transfer_split",
        "sweep_all",
        "relay_tx",
        "submit_transfer",
    ] {
        assert_eq!(
            w.count(method),
            0,
            "inspection must not prepare or broadcast"
        );
    }
}

#[test]
fn native_pending_inspection_reopens_only_the_recorded_wallet_and_keeps_all_uncertainty() {
    let w = World::new("native-inspect");
    w.open_fixture_wallet();
    let ctx = w.ctx.clone().with_wallet_probe_at_refresh();
    let client = fmonero_core::WalletClient::from_ctx(&ctx).unwrap();
    client.close_wallet().unwrap();
    state::clear_last_open(&ctx).unwrap();
    let expected = native_pending_record();
    let bytes = write_native_pending(&ctx, &expected);
    let before = w.count("open_wallet");
    assert!(fmonero_core::wallet_session::open_secret(
        &ctx,
        &client,
        "main",
        fmonero_core::SecretString::empty(),
        &CancelToken::new()
    )
    .is_err());
    assert_eq!(
        w.count("open_wallet"),
        before,
        "normal open must remain guarded"
    );
    let returned = fmonero_core::native_tx::reopen_pending_for_inspection(
        &ctx,
        &client,
        &expected,
        fmonero_core::SecretString::empty(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(returned.wallet_reopened, "{:?}", returned.warning);
    assert_eq!(w.count("open_wallet"), before + 1);
    assert_eq!(
        fmonero_core::wallet_ops::identity(&ctx, &client).unwrap(),
        expected.wallet
    );
    assert_eq!(
        fmonero_core::native_tx::pending(&ctx).unwrap(),
        Some(expected)
    );
    assert_native_guard_unchanged(&w, &bytes);
}

#[test]
fn native_pending_inspection_refuses_stale_identity_network_and_cancel_before_rpc() {
    let w = World::new("native-inspect-stale");
    let ctx = w.ctx.clone().with_wallet_probe_at_refresh();
    let client = fmonero_core::WalletClient::from_ctx(&ctx).unwrap();
    let expected = native_pending_record();
    let bytes = write_native_pending(&ctx, &expected);
    for changed in 0..3 {
        let mut stale = expected.clone();
        match changed {
            0 => stale.wallet.name = "another-wallet".into(),
            1 => stale.artifact_sha256 = "d".repeat(64),
            _ => stale.attempted_at += 1,
        }
        assert!(fmonero_core::native_tx::reopen_pending_for_inspection(
            &ctx,
            &client,
            &stale,
            fmonero_core::SecretString::empty(),
            &CancelToken::new()
        )
        .is_err());
    }
    let mut changed_ctx = ctx.clone();
    changed_ctx.settings.node.network = fmonero_core::settings::Network::Stagenet;
    assert!(fmonero_core::native_tx::reopen_pending_for_inspection(
        &changed_ctx,
        &client,
        &expected,
        fmonero_core::SecretString::empty(),
        &CancelToken::new()
    )
    .is_err());
    let cancelled = CancelToken::new();
    cancelled.cancel();
    assert!(fmonero_core::native_tx::reopen_pending_for_inspection(
        &ctx,
        &client,
        &expected,
        fmonero_core::SecretString::empty(),
        &cancelled
    )
    .is_err());
    assert_eq!(w.count("get_version"), 0);
    assert_eq!(w.count("open_wallet"), 0);
    assert_eq!(w.count("get_address"), 0);
    assert_native_guard_unchanged(&w, &bytes);
}

#[test]
fn native_pending_inspection_rejects_wrong_engine_identity_without_recording_a_session() {
    let w = World::new("native-inspect-mismatch");
    let ctx = w.ctx.clone().with_wallet_probe_at_refresh();
    let client = fmonero_core::WalletClient::from_ctx(&ctx).unwrap();
    let mut expected = native_pending_record();
    expected.wallet.primary_address = "888tNkZrPN6JsEgekjMnABU4TBzc2Dt29EPAvkRxbANsAnjyPbb3iQ1YBRk1UXcdRsiKc9dhwMVgN5S9cQUiyoogDavup3H".into();
    let bytes = write_native_pending(&ctx, &expected);
    let returned = fmonero_core::native_tx::reopen_pending_for_inspection(
        &ctx,
        &client,
        &expected,
        fmonero_core::SecretString::empty(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(!returned.wallet_reopened);
    assert!(returned.warning.is_some());
    assert_eq!(w.count("open_wallet"), 1);
    assert_eq!(w.count("close_wallet"), 1);
    assert_eq!(w.count("set_daemon"), 0);
    assert!(state::last_open(&ctx).unwrap().is_none());
    assert_native_guard_unchanged(&w, &bytes);
}

#[test]
fn native_pending_inspection_retains_record_when_password_open_fails() {
    let w = World::with_wallet(
        "native-inspect-open-failed",
        &["--fail", "open_wallet=-1:Failed to load wallet"],
    );
    let ctx = w.ctx.clone().with_wallet_probe_at_refresh();
    let client = fmonero_core::WalletClient::from_ctx(&ctx).unwrap();
    let expected = native_pending_record();
    let bytes = write_native_pending(&ctx, &expected);
    let returned = fmonero_core::native_tx::reopen_pending_for_inspection(
        &ctx,
        &client,
        &expected,
        fmonero_core::SecretString::new("synthetic-wrong-password".into()).unwrap(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(!returned.wallet_reopened);
    assert!(returned.warning.is_some());
    assert!(!format!("{returned:?}").contains("synthetic-wrong-password"));
    assert_eq!(w.count("open_wallet"), 1);
    assert_eq!(w.count("get_address"), 0);
    assert!(state::last_open(&ctx).unwrap().is_none());
    assert_native_guard_unchanged(&w, &bytes);
}

#[test]
fn native_pending_inspection_does_not_bypass_a_separate_ordinary_pending_record() {
    let w = World::new("native-inspect-ordinary-pending");
    let client = fmonero_core::WalletClient::from_ctx(&w.ctx).unwrap();
    let expected = native_pending_record();
    let bytes = write_native_pending(&w.ctx, &expected);
    let ordinary = PendingSend {
        kind: PendingKind::Send,
        account_index: 0,
        dest: DEST.into(),
        built_at: 1234,
        txs: vec![PendingTx {
            txid: "b".repeat(64),
            amount: Piconero(1),
            fee: Piconero(1),
            relayed: false,
            relayed_at: None,
        }],
    };
    state::set_pending_send(&w.ctx, &ordinary).unwrap();
    assert!(fmonero_core::native_tx::reopen_pending_for_inspection(
        &w.ctx,
        &client,
        &expected,
        fmonero_core::SecretString::empty(),
        &CancelToken::new()
    )
    .is_err());
    assert_eq!(w.count("get_version"), 0);
    assert_eq!(w.count("open_wallet"), 0);
    assert_eq!(
        state::pending_send(&w.ctx).unwrap().unwrap().txs[0].txid,
        ordinary.txs[0].txid
    );
    assert_native_guard_unchanged(&w, &bytes);
}

#[test]
fn native_pending_inspection_form_is_local_masked_and_requires_explicit_review() {
    use super::advanced::Tool;
    for (width, height) in [(40, 10), (70, 18), (120, 40)] {
        for mouse in [false, true] {
            let w = World::new("native-inspect-form");
            let expected = native_pending_record();
            let bytes = write_native_pending(&w.ctx, &expected);
            let mut m = model(&w.ctx, Screen::Tools);
            let _ = m.click_button(Button::Tool(Tool::ReopenNativePending));
            assert!(m.action.is_none());
            let Some(Modal::Workflow(form)) = &mut m.modal else {
                panic!("inspection form");
            };
            assert_eq!(form.identity, Some(expected.wallet.clone()));
            assert_eq!(form.native_pending, Some(expected.clone()));
            assert_eq!(form.fields.len(), 1, "identity cannot be edited");
            assert!(matches!(
                form.fields[0].value,
                super::workflows::Value::Secret(_)
            ));
            form.event(&paste("synthetic-protected-input"));
            assert!(!modal_text(&m, width, height).contains("synthetic-protected-input"));
            for stage in 0..2 {
                if mouse {
                    let at = find(&m, width, height, |t| t == Target::ModalButton(0))
                        .expect("review and confirmation stay clickable");
                    let _ = m.update(Msg::Raw(click(at.0, at.1)));
                } else {
                    let _ = m.on_key(KeyEvent::new(KeyCode::Enter));
                }
                if stage == 0 {
                    assert!(m.action.is_none(), "first activation only reviews");
                    assert!(find(&m, width, height, |t| t == Target::ModalButton(1)).is_some());
                }
            }
            let slot = m.action.as_ref().expect("explicit operation").slot.clone();
            let action = slot.lock().unwrap().take().unwrap();
            assert!(!format!("{action:?}").contains("synthetic-protected-input"));
            let subs::Action::Native(job) = action else {
                panic!("inspection only");
            };
            let super::native::Job::InspectPending {
                expected: selected,
                password,
            } = *job
            else {
                panic!("inspection only");
            };
            assert_eq!(selected, expected);
            assert_eq!(password.expose_secret(), "synthetic-protected-input");
            assert_eq!(w.count("get_version"), 0, "UI has not executed the worker");
            assert_eq!(w.count("open_wallet"), 0);
            assert_native_guard_unchanged(&w, &bytes);
        }
    }
}

mod wallet_prerequisite_tests {
    use super::*;
    use crate::tui::{workflows::Kind, LaunchIntent, Prompt};

    // Default exercises the ordinary in-app New address action, not a launch flag.
    const ROUTES: [LaunchIntent; 3] = [
        LaunchIntent::NewAddress,
        LaunchIntent::Requests,
        LaunchIntent::Default,
    ];
    const SIZES: [(u16, u16); 3] = [(40, 10), (80, 24), (120, 40)];

    fn closed_status(world: &World) -> fmonero_core::Status {
        let mut status = fmonero_core::status::status(&world.ctx).unwrap();
        status.wallet.open = false;
        status.wallet.busy = false;
        status.wallet.name = None;
        status
    }

    fn enter(world: &World, closed: &fmonero_core::Status, route: LaunchIntent) -> Tui {
        let mut m = model(&world.ctx, route.screen());
        m.launch_intent = route;
        // These model tests deliver completed reads explicitly; returned read tasks are not run.
        let _ = m.update(Msg::Status(Box::new(Ok(closed.clone()))));
        let _ = render(&m, 80, 24);
        if route == LaunchIntent::Default {
            let _ = m.update(Msg::Raw(key(KeyCode::Char('n'))));
        }
        assert!(matches!(m.modal, Some(Modal::OpenWallet { .. })));
        assert_eq!(
            m.launch_intent,
            if route == LaunchIntent::Requests {
                LaunchIntent::Requests
            } else {
                LaunchIntent::NewAddress
            }
        );
        assert!(m.action.is_none());
        m
    }

    fn activate(m: &mut Tui, code: KeyCode, target: Target, mouse: bool, size: (u16, u16)) {
        let event = if mouse {
            let at = find(m, size.0, size.1, |t| t == target)
                .expect("the rendered prerequisite control must be reachable");
            click(at.0, at.1)
        } else {
            let _ = render(m, size.0, size.1);
            key(code)
        };
        let _ = m.update(Msg::Raw(event));
    }

    fn cancel(m: &mut Tui, mouse: bool, size: (u16, u16)) {
        activate(m, KeyCode::Escape, Target::ModalButton(1), mouse, size);
    }

    fn stays_dismissed(m: &mut Tui, status: &fmonero_core::Status) {
        assert_eq!(m.launch_intent, LaunchIntent::Default);
        assert!(m.modal.is_none());
        assert!(m.action.is_none());
        for navigate in [false, true] {
            if navigate {
                let _ = m.update(Msg::Raw(key(KeyCode::Char('6'))));
                assert_eq!(m.screen, Screen::Tools);
            }
            let screen = m.screen;
            for _ in 0..3 {
                let _ = m.update(Msg::Status(Box::new(Ok(status.clone()))));
                assert!(
                    m.modal.is_none(),
                    "a later status must not reopen the picker"
                );
                assert!(m.action.is_none(), "a cancelled intent must not arm work");
                assert_eq!(m.screen, screen, "polls must not steal navigation");
            }
        }
        // A late completed address read must not revive Requests either.
        let _ = m.update(Msg::WalletData(
            m.read_epoch,
            Box::new(Ok((Default::default(), Default::default()))),
        ));
        assert!(m.modal.is_none());
        assert!(m.action.is_none());
    }

    fn no_mutations(world: &World) {
        for method in [
            "open_wallet",
            "create_wallet",
            "restore_deterministic_wallet",
            "create_address",
            "transfer_split",
            "relay_tx",
        ] {
            assert_eq!(world.count(method), 0, "cancel must not execute {method}");
        }
    }

    #[test]
    fn wallet_prerequisite_picker_and_password_cancel_survive_polls_and_navigation() {
        let world = World::new("prerequisite-picker-cancel");
        let closed = closed_status(&world);
        for route in ROUTES {
            for asking in [false, true] {
                for mouse in [false, true] {
                    for size in SIZES {
                        for privacy in [false, true] {
                            let mut m = enter(&world, &closed, route);
                            m.ctx.settings.tui.privacy = privacy;
                            if asking {
                                activate(
                                    &mut m,
                                    KeyCode::Enter,
                                    Target::ModalButton(0),
                                    mouse,
                                    size,
                                );
                                let _ = m.update(Msg::Raw(paste("synthetic-protected-input")));
                                assert!(matches!(&m.modal, Some(Modal::OpenWallet {
                                    asking: true, pass, ..
                                }) if !pass.value().is_empty()));
                            }
                            cancel(&mut m, mouse, size);
                            stays_dismissed(&mut m, &closed);
                        }
                    }
                }
            }
        }
        no_mutations(&world);
    }

    #[test]
    fn wallet_prerequisite_empty_picker_cancel_and_close_consume_intent() {
        let world = World::new("prerequisite-empty-close");
        std::fs::remove_file(world.ctx.settings.wallet.dir.join("main.keys")).unwrap();
        let closed = closed_status(&world);
        for route in ROUTES {
            for close in [false, true] {
                for mouse in [false, true] {
                    for size in SIZES {
                        let mut m = enter(&world, &closed, route);
                        assert!(
                            matches!(&m.modal, Some(Modal::OpenWallet { names, .. }) if names.is_empty())
                        );
                        if close {
                            activate(&mut m, KeyCode::Enter, Target::ModalButton(0), mouse, size);
                        } else {
                            cancel(&mut m, mouse, size);
                        }
                        stays_dismissed(&mut m, &closed);
                    }
                }
            }
        }
        no_mutations(&world);
    }

    #[test]
    fn wallet_prerequisite_create_restore_cancel_consumes_intent() {
        let world = World::new("prerequisite-create-restore-cancel");
        std::fs::remove_file(world.ctx.settings.wallet.dir.join("main.keys")).unwrap();
        let closed = closed_status(&world);
        for route in ROUTES {
            for (code, button) in [('c', Button::CreateWallet), ('r', Button::Restore)] {
                for mouse in [false, true] {
                    for size in SIZES {
                        for privacy in [false, true] {
                            let mut m = enter(&world, &closed, route);
                            m.ctx.settings.tui.privacy = privacy;
                            let intent = m.launch_intent;
                            activate(
                                &mut m,
                                KeyCode::Char(code),
                                Target::Button(button),
                                mouse,
                                size,
                            );
                            assert!(
                                matches!(&m.modal, Some(Modal::CreateWallet(_)))
                                    || matches!(&m.modal, Some(Modal::Workflow(form)) if form.kind == Kind::Restore)
                            );
                            assert_eq!(
                                m.launch_intent, intent,
                                "advancing must preserve continuation"
                            );
                            // Cancel after editing a protected field, not just an empty form.
                            let _ = m.update(Msg::Raw(key(KeyCode::Tab)));
                            let _ = m.update(Msg::Raw(paste("synthetic-protected-input")));
                            cancel(&mut m, mouse, size);
                            stays_dismissed(&mut m, &closed);
                        }
                    }
                }
            }
        }
        no_mutations(&world);
    }

    #[test]
    fn wallet_prerequisite_open_resumes_the_intended_form_once() {
        let world = World::new("prerequisite-open-continues");
        world.ctx.settings.save(&world.ctx).unwrap();
        let closed = closed_status(&world);
        for route in ROUTES {
            for mouse in [false, true] {
                let mut m = enter(&world, &closed, route);
                let before = world.count("open_wallet");
                activate(
                    &mut m,
                    KeyCode::Enter,
                    Target::ModalButton(0),
                    mouse,
                    (80, 24),
                );
                let intent = m.launch_intent;
                activate(
                    &mut m,
                    KeyCode::Enter,
                    Target::ModalButton(0),
                    mouse,
                    (80, 24),
                );
                assert_eq!(
                    m.launch_intent, intent,
                    "submitting Open must preserve continuation"
                );
                let action_id = m.action.as_ref().expect("Open is armed").id;
                for _ in 0..3 {
                    let _ = m.update(Msg::Status(Box::new(Ok(closed.clone()))));
                    assert!(
                        m.modal.is_none(),
                        "polls must not reopen the prerequisite during Open"
                    );
                    assert_eq!(m.action.as_ref().unwrap().id, action_id);
                    assert_eq!(m.launch_intent, intent);
                }
                let mut s = ProgramSimulator::new(m);
                run_action(&mut s); // The real UI worker and authenticated mock open sequence.
                if route != LaunchIntent::Requests {
                    run_action(&mut s); // Read identity for the new-address form; no mutation.
                    assert!(matches!(
                        s.model().modal,
                        Some(Modal::Prompt {
                            what: Prompt::NewAddress { .. },
                            ..
                        })
                    ));
                } else {
                    assert!(matches!(&s.model().modal, Some(Modal::Workflow(form))
                        if form.kind == Kind::CreateRequest));
                }
                assert_eq!(world.count("open_wallet"), before + 1);
                let opened = s.model().status.clone().unwrap();
                assert!(opened.wallet.open);
                cancel(s.model_mut(), mouse, (80, 24));
                stays_dismissed(s.model_mut(), &opened);
                assert_eq!(world.count("open_wallet"), before + 1);
                assert_eq!(world.count("create_address"), 0);
                assert_eq!(world.count("transfer_split"), 0);
                assert_eq!(world.count("relay_tx"), 0);
            }
        }
    }

    #[test]
    fn wallet_prerequisite_create_restore_results_resume_after_dismissal_once() {
        use fmonero_core::rpc::wallet::{AddressSet, SubaddressRow};

        let world = World::new("prerequisite-result-continues");
        world.ctx.settings.save(&world.ctx).unwrap();
        std::fs::remove_file(world.ctx.settings.wallet.dir.join("main.keys")).unwrap();
        let closed = closed_status(&world);
        let mut opened = closed.clone();
        opened.wallet.open = true;
        opened.wallet.name = Some("created".into());
        opened.wallet.primary_address = Some(DEST.into());
        for route in ROUTES {
            for restore in [false, true] {
                let mut m = enter(&world, &closed, route);
                let _ = m.update(Msg::Raw(key(KeyCode::Char(if restore {
                    'r'
                } else {
                    'c'
                }))));
                let intent = m.launch_intent;
                // Model scheduling evidence only: these replies do not qualify wallet creation.
                let outcome = if restore {
                    subs::Outcome::Restored(fmonero_core::restore::Restored {
                        name: "created".into(),
                        restore_height: 0,
                        address: Some(DEST.into()),
                        network: Some(fmonero_core::NetType::Mainnet),
                        state: fmonero_core::restore::RestoreState::Ready,
                        warning: None,
                        unit_started: false,
                    })
                } else {
                    subs::Outcome::Created {
                        name: "created".into(),
                    }
                };
                let _ = m.update(Msg::ActionDone(Box::new(Ok(outcome))));
                let _ = m.update(Msg::Status(Box::new(Ok(opened.clone()))));
                let _ = m.update(Msg::WalletData(
                    m.read_epoch,
                    Box::new(Ok((
                        Default::default(),
                        AddressSet {
                            primary: DEST.into(),
                            addresses: vec![SubaddressRow {
                                address: DEST.into(),
                                ..Default::default()
                            }],
                        },
                    ))),
                ));
                assert_eq!(m.launch_intent, intent);
                assert!(m.action.is_none());
                assert!(
                    matches!(m.modal, Some(Modal::SeedBackup { .. } | Modal::Result(_))),
                    "completion information stays open until dismissed"
                );
                let _ = m.update(Msg::Raw(key(KeyCode::Escape)));
                assert_eq!(
                    m.launch_intent, intent,
                    "closing a completed result is not cancellation"
                );
                let _ = m.update(Msg::Status(Box::new(Ok(opened.clone()))));
                if route == LaunchIntent::Requests {
                    assert!(
                        matches!(&m.modal, Some(Modal::Workflow(form)) if form.kind == Kind::CreateRequest)
                    );
                } else {
                    assert!(
                        m.action.is_some(),
                        "resume preparation of the new-address form"
                    );
                    let _ = m.update(Msg::ActionDone(Box::new(Ok(
                        subs::Outcome::NewAddressForm {
                            wallet: fmonero_core::wallet_ops::WalletIdentity {
                                name: "created".into(),
                                network: fmonero_core::NetType::Mainnet,
                                primary_address: DEST.into(),
                            },
                            account: 0,
                        },
                    ))));
                    assert!(matches!(
                        m.modal,
                        Some(Modal::Prompt {
                            what: Prompt::NewAddress { .. },
                            ..
                        })
                    ));
                }
                cancel(&mut m, false, (80, 24));
                stays_dismissed(&mut m, &opened);
            }
        }
        no_mutations(&world);
    }
}
