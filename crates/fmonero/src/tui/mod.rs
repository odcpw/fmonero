//! The FrankenTUI app: Node · Wallet · Send · Log (DESIGN.md §7a).
//!
//! One `Model`, one 1 s `status` poll, one 120 ms tick for the spinner, and at most one long
//! action at a time — which runs in [`subs::ActionSub`] so `Esc` can cancel it (a `Cmd`
//! cannot be cancelled). Short reads go through `Cmd::task`. The TUI owns a `Context` clone
//! and calls `fmonero-core` **in process**; it never spawns `fmonero`, and it never prints:
//! the runtime owns the terminal.
//!
//! Every money guarantee of §2a is a property of `fmonero_core::send`, not of this file: the
//! TUI never calls `build_transfer`, `build_sweep_all` or `relay_tx`, and never writes the
//! `pending.json` record (§2a.3). `u`/`d` go through `node_session`, the open modal through
//! `wallet_session` (§5b), so the §2i single-writer precondition and the `set_daemon` step
//! exist in exactly one place.

mod hit;
mod screens;
mod send;
mod subs;
#[cfg(test)]
mod tests;
mod theme;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use clap::Args;
use ftui::runtime::{Every, SubId, Subscription};
use ftui::widgets::input::TextInput;
use ftui::widgets::modal::{Dialog, DialogState};
use ftui::widgets::table::TableState;
use ftui::widgets::virtualized::VirtualizedListState;
use ftui::widgets::{LogViewer, LogViewerState, LogWrapMode, TabsState};
use ftui::{
    App, Cmd, Event, KeyCode, KeyEvent, KeyEventKind, Model, Modifiers, MouseButton, MouseEvent,
    MouseEventKind, Style,
};

use fmonero_core::nodes::{self, ProbeTarget};
use fmonero_core::rpc::node::{Ban, Connection, TxpoolStats};
use fmonero_core::rpc::wallet::{AddressSet, Balance, BookEntry};
use fmonero_core::{
    history, log as corelog, proc, settings as coresettings, state, status as corestatus, units,
    wallet_dir, Context, Error, HistoryQuery, NodeClient, SendRequest, Status, Transfer,
    TransferKind, WalletClient,
};

use send::{SendCmd, SendScreen, Stage};
use subs::{Action, ActionErr, ActionSub, Outcome};
use theme::Theme;

/// `fmonero tui [--node] [--wallet] [--send] [--log]`.
#[derive(Args, Clone, Debug, Default)]
pub struct TuiArgs {
    /// Open on the Node screen (the default)
    #[arg(long)]
    pub node: bool,
    /// Open on the Wallet screen
    #[arg(long)]
    pub wallet: bool,
    /// Open on the Send screen
    #[arg(long)]
    pub send: bool,
    /// Open on the Log screen
    #[arg(long)]
    pub log: bool,
}

impl TuiArgs {
    fn screen(&self) -> Screen {
        if self.wallet {
            Screen::Wallet
        } else if self.send {
            Screen::Send
        } else if self.log {
            Screen::Log
        } else {
            Screen::Node
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Node,
    Wallet,
    Send,
    Log,
}

impl Screen {
    fn index(self) -> usize {
        match self {
            Screen::Node => 0,
            Screen::Wallet => 1,
            Screen::Send => 2,
            Screen::Log => 3,
        }
    }
    fn from_index(i: usize) -> Screen {
        match i % 4 {
            0 => Screen::Node,
            1 => Screen::Wallet,
            2 => Screen::Send,
            _ => Screen::Log,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Screen::Node => "Node",
            Screen::Wallet => "Wallet",
            Screen::Send => "Send",
            Screen::Log => "Log",
        }
    }
}

/// Which half of the Wallet screen has the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Pane {
    Subaddresses,
    #[default]
    History,
}

/// The help overlay and the behaviour share this one table, so they cannot drift (§7a).
pub const KEYS: &[(&str, &str)] = &[
    // **§7a.1's rule, split in two because one row could not state it truthfully.** `Tab`, `⇧Tab`
    // and `Ctrl+C` are resolved above every screen, focus state and modal branch. But `1 2 3 4`
    // and `q` are printable, and §7a.1's own second bullet gives a focused field **every**
    // printable character: with Amount focused, `2` then `4` types `0.0124` and `q` appends a `q`
    // (DECISIONS: DEFECT C). The dispatcher below, `send.rs:94` and the Send screen's status line
    // already said so — the overlay was the thing that lied.
    (
        "Tab  ⇧Tab  Ctrl+C",
        "screens · quit. Always: in a field, in a pop-up or while something runs. Quit twice to \
         stop a job first.",
    ),
    (
        "1 2 3 4  q",
        "the same two, but only when no field has focus. Esc leaves a field.",
    ),
    ("?", "this help"),
    ("r", "check now"),
    (
        "Esc",
        "close this help, else leave the field, else stop what is running",
    ),
    (
        "mouse",
        "click tabs, rows, fields and every [key] chip. The wheel scrolls. Right-click is Esc.",
    ),
    ("u  d", "Node: start · stop your node"),
    ("b", "Node: peers ⇄ bans"),
    ("L", "Node: change how much your node logs (it asks first)"),
    ("p  f", "Node: check the listed nodes · get a fresh list"),
    (
        "Enter  a  H",
        "Node: use the selected node (it asks first) · add an address · go back to the last one",
    ),
    ("l", "Node: use your own node instead (it asks first)"),
    (
        "o  c  x",
        "Wallet: open a wallet · create one · close the open wallet",
    ),
    (
        "W",
        "Wallet: start the wallet service, or stop it when nothing is open",
    ),
    ("a  l", "Wallet: new address · rename the selected one"),
    ("n", "Wallet: edit the note on the selected payment"),
    (
        "R  Q",
        "Wallet: check for payments · show the address as a QR code",
    ),
    ("/  t", "Wallet: filter rows · change which kinds show"),
    (
        "←→  Enter",
        "Wallet: swap panes · copy an address · open a row",
    ),
    (
        "Send: ↑↓  Enter",
        "next/previous field · check it. Tab leaves the form, Esc leaves a field, Enter goes \
         back in.",
    ),
    (
        "modals: ↑↓  Enter",
        "move between a pop-up's fields · next, then confirm on the last one",
    ),
    (
        "Send: p  Ctrl+P  @",
        "change the fee level (on its row, or from anywhere) · address book",
    ),
    ("Send: ←→", "on the Fee or Account row: change it"),
    (
        "Send: sweep",
        "to send everything, in a terminal: fmonero wallet sweep <address> --dry-run",
    ),
    (
        "Send: type \"send\"",
        "typing the word confirms. Esc discards. Once sent it cannot be taken back.",
    ),
    ("Log: /  f  n N", "search · follow · next/previous match"),
];

/// The four fee levels, by the names the user chooses between (§2o.2). Monero's priority is
/// `0..=4` on the wire, where **0 is the daemon's own alias of 1** (§1b.4) — so there are four
/// names, and a stored `0` renders as the first of them. The Send screen's chips, the Node
/// screen's fee row and the send confirm all read from here, so the three cannot disagree.
pub(super) const FEE_LEVELS: [&str; 4] = ["cheapest", "normal", "fast", "fastest"];

/// A wire priority as its name.
pub(super) fn fee_level_name(priority: u32) -> &'static str {
    FEE_LEVELS[(priority.max(1) - 1).min(3) as usize]
}

/// `p`, `Ctrl+P` and `→` cycle **within the four named levels**: 0 and 1 are the same fee, so a
/// five-step cycle spent one press doing nothing visible.
pub(super) fn next_fee_level(priority: u32) -> u32 {
    if priority >= 4 {
        1
    } else {
        priority.max(1) + 1
    }
}

/// `←`, the other way.
pub(super) fn prev_fee_level(priority: u32) -> u32 {
    match priority {
        0 | 1 => 4,
        n => n - 1,
    }
}

const SPINNER: [&str; 4] = ["⠋", "⠙", "⠸", "⠴"];
/// One sample per `status` poll; §7a asks for a 60-sample rx sparkline.
const RX_SAMPLES: usize = 60;
/// How many history rows the Wallet screen asks core for.
const HISTORY_LIMIT: usize = 500;
/// Lines of the daemon log the Log screen tails.
const LOG_LINES: usize = 2_000;
// §2o.2 says `id`, never `txid` in prose — and this hint is the one place the word survived the
// v5.6 round, because the grep's word list did not carry it (self-read, round 3; the list does now).
pub(super) const FILTER_HINT: &str = "filter by id, address or note";
pub(super) const SEARCH_HINT: &str = "search the log";

/// The node extras the Node screen needs beyond §5's `status` document.
#[derive(Default)]
pub(super) struct NodeExtras {
    pub connections: Vec<Connection>,
    pub txpool: Option<TxpoolStats>,
    pub bans: Vec<Ban>,
}

#[derive(Default)]
pub(super) struct NodeUi {
    pub extras: Option<NodeExtras>,
    pub err: Option<String>,
    pub view: NodeView,
    pub table: RefCell<TableState>,
    pub rx: VecDeque<f64>,
    /// One (instant, height) sample, for the sync ETA.
    sample: Option<(Instant, u64)>,
    eta: Option<u64>,
}

#[derive(Default)]
pub(super) struct WalletUi {
    pub balance: Option<Balance>,
    pub addresses: Option<AddressSet>,
    pub err: Option<String>,
    inflight: bool,
    pub table: RefCell<TableState>,
    pub pane: Pane,
    pub rows: Vec<Transfer>,
    /// The WALLET's height and the local clock `history::query` derived — never the node's
    /// (§1b.4). `None` means `confirmations` and `locked` render `—`.
    pub wh: Option<u64>,
    pub now: Option<i64>,
    pub rows_err: Option<String>,
    pub list: RefCell<VirtualizedListState>,
    pub filter: TextInput,
    pub filter_open: bool,
    pub kind: Option<TransferKind>,
    pub detail: bool,
}

/// The public-node picker's own state (§7a.4, v5.5).
#[derive(Default)]
pub(super) struct PickerUi {
    /// The cache as `nodes::cached` read it — `None` means no cache, which renders §7a.4's
    /// no-cache panel and not an empty table with a broken footer (revision 8).
    pub list: Option<fmonero_core::NodeList>,
    /// **No row is selected on entry** (revision 4): with row 0 selected and the configured
    /// node sorted first, a mouse user's first click here would activate the node they are
    /// already using and §2m.5 step 9 would close an open wallet for a change of nothing.
    pub table: RefCell<TableState>,
    pub filter: TextInput,
    pub filter_open: bool,
    pub err: Option<String>,
    /// `fetching…` / `probing 7/20 · Esc to stop`, so the thing that contacts strangers is
    /// visible while it happens (§2n.5).
    pub progress: Option<String>,
    /// `H`: the address configured before the last switch, for the session only. A "previous
    /// node" that survived a restart would be a fourth piece of node state nobody asked for.
    pub prev: Option<String>,
    /// When the configured node was last probed on the §2n.4 cadence.
    pub probed_at: Option<Instant>,
}

pub(super) struct LogUi {
    pub viewer: RefCell<LogViewer>,
    pub state: RefCell<LogViewerState>,
    pub path: PathBuf,
    pub empty: bool,
    pub follow: bool,
    /// The TUI's own 0→4→0 cycle pointer for `L`: the daemon never reports its level.
    level: i8,
    pub search: TextInput,
    pub search_open: bool,
}

/// Which table the Node screen's list pane shows (`b`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum NodeView {
    #[default]
    Peers,
    Bans,
}

pub(super) enum Modal {
    Confirm {
        what: Confirm,
        dialog: Box<Dialog>,
        state: RefCell<DialogState>,
    },
    OpenWallet {
        names: Vec<String>,
        sel: usize,
        /// Masked (§2b.4): the buffer is overwritten with zeroes when the modal closes.
        pass: TextInput,
        asking: bool,
    },
    Prompt {
        what: Prompt,
        title: String,
        input: TextInput,
    },
    Qr {
        address: String,
    },
    /// v5.5, §7a.5. Four controls — name, password, password again, language — moving with
    /// `Up`/`Down`, `Enter` advancing from every one but the last where it submits (§7a.1,
    /// revision 1). Without a field key this modal is a trap: it is one of the exactly two that
    /// consume `Tab`, so a keyboard-only user would type a name and be stuck, which is the
    /// complaint (4) flow made unreachable by the fix for complaint (1).
    CreateWallet(Box<CreateForm>),
    /// v5.5, §7a.5. Dismiss-only, carrying `seed_backup_hint` verbatim from core — the same
    /// string the CLI prints, not a second wording. **fmonero never displays a seed.**
    SeedBackup {
        lines: Vec<String>,
    },
    /// v5.5, §7a.4. The picker's safety surface: the full address, `untrusted` as a fact, and
    /// the three consequences before the fact.
    UseNode {
        address: String,
        /// The wallet that is open and will be closed by the restart, named when there is one.
        wallet_open: Option<String>,
    },
    /// v5.5, §7a.4. One field, validated live by `Settings::parse_remote_address` — the same
    /// function the CLI uses, so the TUI cannot accept a string the CLI refuses. **No
    /// `--trusted` toggle**: `fmonero node remote <addr> --trusted --yes` is the documented
    /// place, and a path to request trusted from this screen would make the confirm modal's
    /// rendered `untrusted` line an untruth (revision 5).
    AddNode {
        input: TextInput,
    },
    /// v5.5, §2n.5 rule 2. Shown before the first fetch **or** the first probe while the
    /// cache's `notice_ack` is unset, and `Enter` writes that field.
    Notice {
        /// What the user was trying to do, resumed on `Enter`.
        then: NoticeThen,
    },
}

/// §7a.5's create-wallet form. **Boxed** in [`Modal`]: four `TextInput`s beside a one-word
/// `Confirm` variant is the size difference clippy objects to, and the box costs one allocation
/// per modal open.
pub(super) struct CreateForm {
    pub name: TextInput,
    /// Masked (§2b.4): both buffers are overwritten with zeroes when the modal closes.
    pub pass: TextInput,
    pub again: TextInput,
    /// An index into `settings::SEED_LANGUAGES`, rendered `◂ English ▸`.
    pub lang: usize,
    /// 0 name · 1 password · 2 password again · 3 language.
    pub field: usize,
    pub err: Option<String>,
}

/// What §2n.5's notice was standing in front of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoticeThen {
    Fetch,
    Probe,
}

#[derive(Clone, Copy)]
pub(super) enum Confirm {
    NodeUp,
    NodeDown,
    /// The next level in the 0–4 cycle. A mutation, so it asks first (§7a).
    LogLevel(i8),
    /// v5.6, §7a.4's `l`: go back to your own node. It asks first for the same reason
    /// `Confirm::UseNode` does — it restarts the wallet service and can close the wallet this
    /// session is holding (§2m.5).
    NodeLocal,
}

pub(super) enum Prompt {
    Label { index: u32 },
    Note { txid: String },
}

struct ActionSlot {
    id: SubId,
    slot: Arc<Mutex<Option<Action>>>,
    label: String,
    started: Instant,
    deadline_sec: u64,
    cancellable: bool,
    /// [`Action::wallet_busy`], recorded here because the `Action` moves into `slot` and the
    /// subscription takes it (FINDING D).
    wallet_busy: bool,
    /// This action is the node starting (W9), for the same reason and recorded the same way: the
    /// hero says `Starting your node…` while it runs, instead of advising the key that is running.
    starting_node: bool,
}

/// The rows `history::query` returned, plus the WALLET height and local clock it derived —
/// the two `Option`s §1b.4 forbids a caller from supplying.
type HistoryRead = (Vec<Transfer>, Option<u64>, Option<i64>);

pub enum Msg {
    Raw(Event),
    /// 1 s: re-read `status` (dropped when one is already in flight, §2d).
    Poll,
    /// 120 ms: the spinner frame budget and the note expiry. It touches no daemon.
    Tick,
    Status(Box<Result<Status, String>>),
    Extras(Box<Result<NodeExtras, String>>),
    WalletData(Box<Result<(Balance, AddressSet), String>>),
    History(Box<Result<HistoryRead, String>>),
    LogLines(Box<Result<(Vec<String>, PathBuf), String>>),
    Book(Vec<BookEntry>),
    /// `validate_address` + the nettype check, before the Send screen builds (§7a step 2).
    Validated(Box<Result<SendRequest, String>>),
    /// A short RPC: new subaddress, label, note, log level, or a clipboard copy.
    Task(&'static str, Box<Result<String, ActionErr>>),
    ActionDone(Box<Result<Outcome, ActionErr>>),
}

impl From<Event> for Msg {
    fn from(e: Event) -> Self {
        Msg::Raw(e)
    }
}

pub struct Tui {
    ctx: Context,
    exit: Arc<AtomicI32>,
    theme: Theme,
    theme_mtime: Option<SystemTime>,
    /// v5.6, §7a.7. `config.toml`'s modification time as of the last time this model read it, so
    /// a settings change made **anywhere else** — the CLI in another terminal, an editor, a
    /// `doctor --fix` that repaired a key — is noticed on the 1 s poll instead of being invisible
    /// until the TUI is restarted. `stat` on one file per second is not a cost worth optimising.
    config_mtime: Option<SystemTime>,
    screen: Screen,
    tabs: RefCell<TabsState>,
    show_help: bool,
    tick: usize,
    status: Option<Status>,
    status_err: Option<String>,
    poll_inflight: bool,
    node: NodeUi,
    /// v5.5, §7a.4.
    picker: PickerUi,
    wallet: WalletUi,
    logs: LogUi,
    send: SendScreen,
    modal: Option<Modal>,
    /// `send::relay`'s announce sink (§5b): core is silent, so the line naming the recipient,
    /// amount, fee, total, priority and account lands in the Log pane.
    announce: Arc<Mutex<Vec<String>>>,
    action: Option<ActionSlot>,
    sub_seq: u64,
    quit_armed: Option<(Instant, i32)>,
    note: String,
    note_err: bool,
    note_at: Instant,
    /// v5.5, §7a.2: the rects `view()` laid out, as `update()` needs them.
    hits: hit::Map,
    /// The wallets in `wallet.dir`, re-read on entry to the Wallet screen so the empty state
    /// can say `0 wallets` — a real count, so a user can see why `o` will not help before
    /// pressing it (§7a.5).
    wallets: Vec<String>,
}

/// Run the TUI. `Ok(0)` on a clean quit, the §4a code of the session's last failed action
/// otherwise, `Ok(130)` on Ctrl+C — the TUI never invents an exit code (§7a).
pub fn run(args: TuiArgs, ctx: Context) -> anyhow::Result<i32> {
    let exit = Arc::new(AtomicI32::new(0));
    // §7a.2. `MouseCapturePolicy::Auto` already resolves to *on* in `AltScreen`, which is what
    // `App::fullscreen` uses — so a fullscreen ftui app arguably had capture all along and simply
    // threw the events away. `.with_mouse()` is written explicitly because relying on `Auto` would
    // make the behaviour a consequence of the screen mode rather than a decision, and
    // `tui.mouse = false` has to be able to turn it off: mouse capture takes the terminal's own
    // selection and scrollback away for as long as the TUI runs, and which modifier gives them
    // back differs per terminal.
    let mouse = ctx.settings.tui.mouse;
    let app = Tui::new(args.screen(), ctx, exit.clone());
    let builder = App::fullscreen(app);
    let builder = if mouse {
        builder.with_mouse()
    } else {
        builder.with_mouse_enabled(false)
    };
    builder.run()?;
    Ok(exit.load(Ordering::SeqCst))
}

impl Tui {
    fn new(screen: Screen, ctx: Context, exit: Arc<AtomicI32>) -> Tui {
        let theme = Theme::load();
        let mut send = SendScreen::new(
            &theme,
            ctx.settings.wallet.priority,
            ctx.settings.wallet.account,
        );
        if screen == Screen::Send {
            send.resume();
        }
        Tui {
            theme_mtime: std::fs::metadata(theme::colors_path())
                .and_then(|m| m.modified())
                .ok(),
            config_mtime: std::fs::metadata(ctx.paths.config_file())
                .and_then(|m| m.modified())
                .ok(),
            node: NodeUi::default(),
            picker: PickerUi {
                filter: text_input(&theme),
                ..PickerUi::default()
            },
            wallet: WalletUi {
                filter: text_input(&theme),
                ..WalletUi::default()
            },
            logs: LogUi {
                viewer: RefCell::new(LogViewer::new(LOG_LINES).wrap_mode(LogWrapMode::NoWrap)),
                state: RefCell::new(LogViewerState::default()),
                path: ctx.settings.node.log_file.clone(),
                empty: true,
                follow: true,
                level: 0,
                search: text_input(&theme),
                search_open: false,
            },
            send,
            ctx,
            exit,
            tabs: RefCell::new(TabsState {
                active: screen.index(),
                offset: 0,
            }),
            theme,
            screen,
            show_help: false,
            tick: 0,
            status: None,
            status_err: None,
            poll_inflight: false,
            modal: None,
            announce: Arc::new(Mutex::new(Vec::new())),
            action: None,
            sub_seq: 0,
            quit_armed: None,
            note: String::new(),
            note_err: false,
            note_at: Instant::now(),
            hits: hit::Map::default(),
            wallets: Vec::new(),
        }
    }

    // ------------------------------------------------------------ what the screens ask

    pub(super) fn account_index(&self) -> u32 {
        self.status
            .as_ref()
            .map_or(self.ctx.settings.wallet.account, |s| s.wallet.account_index)
    }

    /// The node badge as **the word a person reads** and the **tone the theme paints it**
    /// (§2o rule 8, v5.6). The two were one string until v5.6, which is why the badge said
    /// `down` for a node that may be perfectly well and merely unreachable: the word had to
    /// double as `Theme::badge`'s style key. The tone keys are internal and never rendered.
    fn node_state(&self) -> (&'static str, &'static str) {
        let Some(s) = &self.status else {
            return ("not answering", "down");
        };
        if self.status_err.is_some() || s.pending.is_some() || self.has_warning("node_stalled") {
            return ("problem", "error");
        }
        // Remote mode answers first (§2m.3): `reachable` and `synchronized` are both `null`
        // there, so every badge below would paint "not answering" for a wallet that is working.
        if self.remote_mode() {
            return ("public node", "up");
        }
        match (s.node.reachable, s.node.synchronized) {
            (Some(true), Some(true)) => ("synced", "up"),
            (Some(true), _) => ("syncing", "syncing"),
            // **V14: one fact, one phrasing.** The hero said `Your node is not running` while the
            // status bar in the same frame said `node not answering`, and a user cannot tell
            // whether those are one problem or two. They are one, and which words are true depends
            // on whether the service is up: a service that is not running is not running, and one
            // that is running and silent is not answering. [`Tui::node_off_words`] decides it once
            // and both surfaces read it.
            _ => (self.node_off_words(), "down"),
        }
    }

    /// The words for a node that is not answering — **three of them now** (W9), from one
    /// predicate, read by the status bar's badge, the Node screen's hero, the three panes that
    /// stand in for a missing answer and (through [`fmonero_core::node_off_words`]) the Notes row.
    ///
    /// `starting` is the TUI's own third state: core cannot know that this program is at this
    /// moment running `u`. Without it the hero said `Your node is not answering` over
    /// `Your node is not running. Press u to start it.` while the status bar ran
    /// `⠸ starting the node 3s / 30s` — three claims about one node, one of them advising the key
    /// that was already running.
    pub(super) fn node_off_words(&self) -> &'static str {
        if self.action.as_ref().is_some_and(|a| a.starting_node) {
            return "starting";
        }
        match self.status.as_ref() {
            Some(s) => fmonero_core::node_off_words(s),
            None => "not answering",
        }
    }

    /// The same three words as a sentence, for the panes that have no answer to draw (W9).
    pub(super) fn node_off_sentence(&self) -> String {
        match self.node_off_words() {
            "starting" => "Your node is starting up.".to_string(),
            words => format!("Your node is {words}."),
        }
    }

    pub(super) fn node_badge(&self) -> &'static str {
        self.node_state().0
    }

    pub(super) fn node_tone(&self) -> &'static str {
        self.node_state().1
    }

    /// `node.mode == remote` — the one discriminator the whole Node screen branches on (§2m).
    pub(super) fn remote_mode(&self) -> bool {
        self.status
            .as_ref()
            .map_or(self.ctx.settings.node.mode.is_remote(), |s| {
                s.node.layout == fmonero_core::units::Layout::Remote
            })
    }

    /// The wallet badge, word and tone (see [`Tui::node_state`]).
    fn wallet_state(&self) -> (&'static str, &'static str) {
        let Some(s) = &self.status else {
            return ("not running", "down");
        };
        if self.has_warning("wallet_stalled") {
            return ("problem", "error");
        }
        let ours = self.action.as_ref().is_some_and(|a| a.wallet_busy);
        match (s.wallet.reachable, s.wallet.open) {
            // **`idle`, not `down`, when §5 says `wallet_idle`** (v5.5). The daemon is DESIGNED to
            // be down until a wallet is opened (§4.2 writes both units disabled), and this line is
            // the one a user reads on every screen — telling them `down` there while §5 calls the
            // same state an *info* and §6.4 keeps the bar icon calm is the drift §2j is about, in
            // the most-read place on the screen.
            (false, _) if self.has_warning("wallet_idle") => ("closed", "down"),
            // **`starting`/`busy`, not `down`, while one of our own actions is the reason a poll
            // cannot answer** (DECISIONS: FINDING D). `W` and a TUI create both ran under a badge
            // reading `down` two lines below a panel reading `starting…`: an assertion about a
            // daemon this model knew to be false. `down` survives only for the state it describes
            // — nothing of ours in flight and the poll still failing, which core has already split
            // into `wallet_idle` (deliberately not running, above) and `wallet_down`.
            (false, _) if ours && s.wallet.unit_active => ("scanning", "syncing"),
            (false, _) if ours => ("starting", "syncing"),
            // **V13: the service is running and it is not answering, and the screen said both.**
            // The Wallet screen read `wallet service: running` (which is `unit_active`) four rows
            // above a status bar reading `wallet not running` (which was `reachable`), and both
            // were computed correctly. `not running` is now kept for the state it describes, and a
            // service that is up without answering says so — the same split §2c makes between
            // `unreachable` and a service that is simply not there.
            (false, _) if s.wallet.unit_active => ("not answering", "down"),
            (false, _) => ("not running", "down"),
            (true, true) => ("open", "up"),
            (true, false) => ("catching up", "syncing"),
        }
    }

    pub(super) fn wallet_badge(&self) -> &'static str {
        self.wallet_state().0
    }

    pub(super) fn wallet_tone(&self) -> &'static str {
        self.wallet_state().1
    }

    pub(super) fn node_phrase(&self) -> String {
        let Some(s) = &self.status else {
            return "checking…".into();
        };
        if self.remote_mode() {
            return format!("Public node · {}", s.node.url);
        }
        if s.node.reachable != Some(true) {
            // V14, and W9's third word: the same predicate the badge and the panes read. While
            // `u` runs the hero says so rather than naming the key that is running.
            if self.node_off_words() == "starting" {
                return "Starting your node…".into();
            }
            return format!("Your node is {}", self.node_off_words());
        }
        match (s.node.synchronized, s.node.sync_percent) {
            (Some(true), _) => "Synced".into(),
            (_, Some(p)) => format!("Syncing {p:.0}%"),
            _ => "Running".into(),
        }
    }

    pub(super) fn monero_missing(&self) -> bool {
        self.has_warning("monero_not_installed")
    }

    pub(super) fn node_restricted(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.node.restricted == Some(true))
    }

    fn has_warning(&self, id: &str) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.warnings.iter().any(|w| w.id == id))
    }

    /// The §2d breaker's own words, so the persistent row and the refusal cannot drift.
    fn breaker_note(&self, id: &str) -> Option<String> {
        self.status
            .as_ref()?
            .warnings
            .iter()
            .find(|w| w.id == id)
            // §2o.1: the in-app sentence, never the shell command. `action` is `None` where
            // nothing can be done from in here, and then the text stands alone.
            .map(|w| match w.action.as_deref() {
                Some(a) => format!("{}  {a}", w.text),
                None => w.text.clone(),
            })
    }

    pub(super) fn sync_eta(&self) -> Option<String> {
        let sec = self.node.eta?;
        Some(screens::age(sec).replace(" ago", ""))
    }

    pub(super) fn visible_history(&self) -> Vec<&Transfer> {
        let needle = self.wallet.filter.value().trim().to_lowercase();
        self.wallet
            .rows
            .iter()
            .filter(|t| self.wallet.kind.is_none_or(|k| t.kind == k))
            .filter(|t| {
                needle.is_empty()
                    || t.txid.to_lowercase().contains(&needle)
                    || t.address.to_lowercase().contains(&needle)
                    || t.note.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub(super) fn selected_transfer(&self) -> Option<&Transfer> {
        let i = self.wallet.list.borrow().selected?;
        self.visible_history().get(i).copied()
    }

    fn selected_address(&self) -> Option<String> {
        let set = self.wallet.addresses.as_ref()?;
        match self.wallet.pane {
            Pane::Subaddresses => {
                let i = self.wallet.table.borrow().selected.unwrap_or(0);
                set.addresses.get(i).map(|a| a.address.clone())
            }
            Pane::History => Some(set.primary.clone()),
        }
    }

    // ------------------------------------------------------------ short reads

    fn poll(&mut self) -> Cmd<Msg> {
        if self.poll_inflight {
            return Cmd::none();
        }
        self.poll_inflight = true;
        let ctx = self.ctx.clone();
        Cmd::task(move || {
            Msg::Status(Box::new(
                corestatus::status(&ctx).map_err(|e| e.to_string()),
            ))
        })
    }

    fn load_extras(&self) -> Cmd<Msg> {
        // Nothing to load in remote mode (§2m.3): every extra needs a `NodeClient`, which
        // `from_ctx` refuses there, and the remote hero renders none of them. `Cmd::none()`
        // rather than a failing task: no error about a configuration that is correct.
        if self.remote_mode() {
            return Cmd::none();
        }
        let ctx = self.ctx.clone();
        let bans = self.node.view == NodeView::Bans;
        Cmd::task(move || {
            let read = || -> Result<NodeExtras, Error> {
                let n = NodeClient::from_ctx(&ctx)?;
                // A restricted node does not register `get_connections`/`get_bans` at all
                // (§1b.2); the panes say so rather than rendering zeros. Every other failure
                // **propagates**: swallowing it made the Peers pane print "no connections yet"
                // and the mempool pane sit on "reading the transaction pool…" for ever beside a
                // hero that had already said the node is not running — two panes describing a
                // state the same screen contradicts, which §2j forbids.
                let restricted = n.is_restricted()?;
                Ok(NodeExtras {
                    connections: if restricted {
                        Vec::new()
                    } else {
                        n.get_connections()?
                    },
                    txpool: Some(n.get_txpool_stats()?),
                    bans: if bans && !restricted {
                        n.get_bans()?
                    } else {
                        Vec::new()
                    },
                })
            };
            Msg::Extras(Box::new(read().map_err(|e| e.to_string())))
        })
    }

    fn load_wallet(&mut self) -> Cmd<Msg> {
        if self.wallet.inflight {
            return Cmd::none();
        }
        self.wallet.inflight = true;
        let ctx = self.ctx.clone();
        let account = self.account_index();
        Cmd::task(move || {
            let read = || -> Result<(Balance, AddressSet), Error> {
                let w = WalletClient::from_ctx(&ctx)?;
                Ok((w.get_balance(account)?, w.get_address(account)?))
            };
            Msg::WalletData(Box::new(read().map_err(|e| e.to_string())))
        })
    }

    fn load_history(&self) -> Cmd<Msg> {
        let ctx = self.ctx.clone();
        let account = self.account_index();
        let kinds = self.wallet.kind.map_or_else(
            || {
                vec![
                    TransferKind::In,
                    TransferKind::Out,
                    TransferKind::Block,
                    TransferKind::Pending,
                    TransferKind::Failed,
                ]
            },
            |k| vec![k],
        );
        Cmd::task(move || {
            let read = || -> Result<HistoryRead, Error> {
                let w = WalletClient::from_ctx(&ctx)?;
                let mut q = HistoryQuery {
                    account_index: account,
                    limit: HISTORY_LIMIT,
                    kinds,
                    ..HistoryQuery::default()
                };
                let rows = history::query(&ctx, Some(&w), &mut q)?;
                // `wallet_height` and `adjusted_now` are written by `query` itself, which is
                // what keeps the NODE's height out of `confirmations` (§1b.4).
                Ok((rows, q.wallet_height, q.adjusted_now))
            };
            Msg::History(Box::new(read().map_err(|e| e.to_string())))
        })
    }

    fn load_log(&self) -> Cmd<Msg> {
        let path = self.logs.path.clone();
        Cmd::task(move || {
            let read = corelog::tail(&path, LOG_LINES, None)
                .map(|t| (t.lines, t.path))
                .map_err(|e| e.to_string());
            Msg::LogLines(Box::new(read))
        })
    }

    fn load_book(&self) -> Cmd<Msg> {
        let ctx = self.ctx.clone();
        Cmd::task(move || {
            let entries = WalletClient::from_ctx(&ctx)
                .and_then(|w| w.get_address_book())
                .unwrap_or_default();
            Msg::Book(entries)
        })
    }

    /// §7a step 2: one `validate_address` plus the nettype check against the **wallet's own**
    /// nettype (§2a.6), before `send::build` runs the same checks again on the money path.
    fn validate_then_build(&self, req: SendRequest) -> Cmd<Msg> {
        let ctx = self.ctx.clone();
        Cmd::task(move || {
            let check = || -> Result<SendRequest, Error> {
                if req.dest.starts_with('@') {
                    // A label is resolved by `send::build` at step 5a, on the open wallet.
                    return Ok(req.clone());
                }
                let w = WalletClient::from_ctx(&ctx)?;
                let v = w.validate_address(&req.dest)?;
                if !v.valid {
                    return Err(Error::invalid_address(format!("{} is not valid", req.dest)));
                }
                if let Some((_, net)) = state::last_open(&ctx)? {
                    if !v.nettype.is_empty() && v.nettype != net.as_str() {
                        return Err(Error::invalid_address(format!(
                            "{} is a {} address; this wallet is {net}",
                            req.dest, v.nettype
                        )));
                    }
                }
                Ok(req.clone())
            };
            Msg::Validated(Box::new(check().map_err(|e| e.to_string())))
        })
    }

    /// One short wallet RPC, named for the status line.
    fn task(
        &self,
        label: &'static str,
        f: impl FnOnce(&Context) -> Result<String, Error> + Send + 'static,
    ) -> Cmd<Msg> {
        let ctx = self.ctx.clone();
        Cmd::task(move || Msg::Task(label, Box::new(f(&ctx).map_err(ActionErr::from))))
    }

    // ------------------------------------------------------------ long actions

    fn start(&mut self, action: Action) -> Cmd<Msg> {
        self.try_start(action).unwrap_or_else(|_| Cmd::none())
    }

    /// Put an action in flight without a subscription behind it, so a test can read the frame the
    /// user sees while one runs (W9). The same `start` the keys call.
    #[cfg(test)]
    pub(super) fn start_for_test(&mut self, action: Action) {
        let _ = self.start(action);
    }

    /// `start`, but it hands the action **back** when it refuses.
    ///
    /// Both guards below consume the `Action` and send no `Msg::ActionDone`, so a caller that
    /// has already advanced the §7a send state machine — `SendCmd::Relay` replaces `Built` with
    /// `Relaying`, `Msg::Validated` replaces `Form` with `Building` — would be left in a stage
    /// only `ActionDone` can leave, with the built `Plan` (and its §2a.11 lock) dropped and
    /// nothing on screen saying a transaction had existed. Those two callers put it back.
    fn try_start(&mut self, action: Action) -> Result<Cmd<Msg>, Box<Action>> {
        if let Some(a) = &self.action {
            self.note(format!("Already {}. Esc cancels it.", a.label), true);
            return Err(Box::new(action));
        }
        let daemon = match action {
            Action::NodeUp | Action::NodeDown => Some("node_stalled"),
            // §2d: "**`send::relay` likewise never consults a breaker**: once a plan is built
            // the §2a.11 `send.lock` is held and the transaction exists, so exiting 5 there
            // would abandon a built transaction rather than protect anything." This gate reads
            // a status poll that is up to a poll-interval stale, so it could only ever refuse
            // the relay of a transaction that already exists.
            Action::Relay(_) => None,
            _ => Some("wallet_stalled"),
        };
        if let Some(text) = daemon.and_then(|id| self.breaker_note(id)) {
            self.note(text, true);
            return Err(Box::new(action));
        }
        self.sub_seq += 1;
        self.note(action.label(), false);
        self.action = Some(ActionSlot {
            id: 0x464d_0000 + self.sub_seq,
            label: action.label(),
            deadline_sec: action.deadline_sec(&self.ctx),
            cancellable: action.cancellable(),
            wallet_busy: action.wallet_busy(),
            starting_node: matches!(action, Action::NodeUp),
            started: Instant::now(),
            slot: Arc::new(Mutex::new(Some(action))),
        });
        Ok(Cmd::none())
    }

    /// `Esc`, or `q` twice: drop the subscription, which raises the `StopSignal`, which trips
    /// the `CancelToken`. A relay is never cancelled (§7a).
    fn cancel_action(&mut self) -> Cmd<Msg> {
        match self.action.take() {
            // W16: `Nothing is running.` on a screen whose subject is a node and a wallet
            // service reads as a claim about those two. `Esc` cancels an action of fmonero's own,
            // so that is what it says it found none of.
            None => self.note("Nothing to cancel.", false),
            Some(a) if !a.cancellable => {
                let label = a.label.clone();
                self.action = Some(a);
                self.note(
                    format!("{label} cannot be stopped. The payment has already gone out."),
                    true,
                );
            }
            Some(a) => self.note(format!("{} stopped.", a.label), true),
        }
        Cmd::none()
    }

    // ------------------------------------------------------- the public-node picker (§7a.4)

    /// Re-read the cache. **Spawns nothing** (`nodes::cached`), which is what lets the Node
    /// screen draw the table on entry and on every change without a fetch on the render path.
    fn reload_picker(&mut self) {
        match nodes::cached(&self.ctx) {
            Ok(list) => {
                self.picker.list = list;
                self.sort_picker();
                self.picker.err = None;
            }
            Err(e) => self.picker.err = Some(e.to_string()),
        }
    }

    /// §2n.6's display order, applied wherever the list changes so one sort serves the whole
    /// screen (`nodes::sort_for_display`, §2a.3).
    fn sort_picker(&mut self) {
        let configured = nodes::configured_address(&self.ctx);
        if let Some(list) = self.picker.list.as_mut() {
            nodes::sort_for_display(&mut list.nodes, configured.as_deref());
        }
    }

    /// The rows the picker is showing, after the filter — the set `p` probes and `Enter` picks
    /// from, so what is on screen and what is acted on cannot differ.
    fn picker_rows(&self) -> Vec<&fmonero_core::PublicNode> {
        let needle = self.picker.filter.value().trim().to_ascii_lowercase();
        self.picker
            .list
            .as_ref()
            .map(|l| {
                l.nodes
                    .iter()
                    .filter(|n| {
                        needle.is_empty() || n.address.to_ascii_lowercase().contains(&needle)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `f`. The §2n.5 notice comes first while the cache's `notice_ack` is unset — before the
    /// first fetch **or** the first probe, because they disclose to different parties.
    fn start_fetch(&mut self, refresh: bool) -> Cmd<Msg> {
        if !nodes::notice_acknowledged(&self.ctx) {
            self.modal = Some(Modal::Notice {
                then: NoticeThen::Fetch,
            });
            return Cmd::none();
        }
        self.picker.progress = Some("fetching the list from monero.fail…".into());
        self.start(Action::NodeListFetch { refresh })
    }

    /// `p`. Probes the rows **visible after the filter**, capped at `nodes::DEFAULT_LIMIT`.
    ///
    /// The cap is on the screen as well as on the CLI because a filter matching 150 rows plus one
    /// keypress is otherwise 150 strangers learning this machine's IP address from one `p`
    /// (§2n.5). Probing is the only thing on this screen that contacts a node the user has not
    /// chosen, which is why the progress row exists.
    fn start_probe(&mut self) -> Cmd<Msg> {
        if !nodes::notice_acknowledged(&self.ctx) {
            self.modal = Some(Modal::Notice {
                then: NoticeThen::Probe,
            });
            return Cmd::none();
        }
        let targets: Vec<ProbeTarget> = self
            .picker_rows()
            .iter()
            .take(nodes::DEFAULT_LIMIT)
            .map(|n| ProbeTarget::from_node(n))
            .collect();
        if targets.is_empty() {
            self.note("No nodes to check. Press f to get the list.", true);
            return Cmd::none();
        }
        self.picker.progress = Some(format!("probing {} nodes · Esc to stop", targets.len()));
        self.start(Action::NodeProbe { targets })
    }

    /// `Enter` on the picker. Two guards, because either alone leaves a bad shape (revision 4):
    /// nothing is selected on entry, so a first click can only select; and the row already
    /// configured is refused with a toast rather than opening the confirm — otherwise a stray
    /// click would restart the wallet unit and close an open wallet for a change of nothing.
    fn use_selected_node(&mut self) -> Cmd<Msg> {
        let Some(sel) = self.picker.table.borrow().selected else {
            self.note("Select a node first, with ↑↓ or a click, then Enter.", true);
            return Cmd::none();
        };
        let Some(address) = self.picker_rows().get(sel).map(|n| n.address.clone()) else {
            return Cmd::none();
        };
        if nodes::configured_address(&self.ctx).as_deref() == Some(address.as_str()) {
            self.note("This is the node you are using.", false);
            return Cmd::none();
        }
        let wallet_open = self
            .status
            .as_ref()
            .and_then(|s| s.wallet.name.clone())
            .filter(|_| self.status.as_ref().is_some_and(|s| s.wallet.open));
        self.modal = Some(Modal::UseNode {
            address,
            wallet_open,
        });
        Cmd::none()
    }

    /// `H` — re-apply the address configured before the last switch, through the same confirm
    /// modal. It is deliberately **not** `node local`: the recovery the v5.4 objection asked for
    /// is a one-key undo of the switch just made, not a mode change (§7a.3).
    fn previous_node(&mut self) -> Cmd<Msg> {
        let Some(address) = self.picker.prev.clone() else {
            self.note("No earlier node to go back to.", true);
            return Cmd::none();
        };
        let wallet_open = self.status.as_ref().and_then(|s| s.wallet.name.clone());
        self.modal = Some(Modal::UseNode {
            address,
            wallet_open,
        });
        Cmd::none()
    }

    /// v5.6, §7a.4's `l` and §2o.1's destination for the command the old refusal printed.
    ///
    /// It goes through a confirm modal rather than acting, for the reason `Confirm::UseNode`
    /// does: the switch re-renders the wallet service and closes the wallet this session may be
    /// holding (§2m.5). With your own node already in force it is a no-op with a sentence.
    fn use_local(&mut self) -> Cmd<Msg> {
        if !self.remote_mode() {
            self.note("You are already using your own node.", false);
            return Cmd::none();
        }
        self.confirm_local()
    }

    /// The §2n.4 cadence: probe **the configured node** on entering the Node screen and every
    /// `node.remote_probe_sec` seconds **while that screen is showing**. It is the only automatic
    /// probe in this design, and it never needs the §2n.5 notice: it is the node already in use.
    fn maybe_probe_configured(&mut self) -> Cmd<Msg> {
        if self.screen != Screen::Node || self.action.is_some() {
            return Cmd::none();
        }
        let every = self.ctx.settings.node.remote_probe_sec;
        if every == 0 {
            return Cmd::none();
        }
        let Some(address) = nodes::configured_address(&self.ctx) else {
            return Cmd::none();
        };
        let due = match self.picker.probed_at {
            None => true,
            Some(at) => at.elapsed() >= Duration::from_secs(u64::from(every)),
        };
        if !due {
            return Cmd::none();
        }
        let Ok(target) = ProbeTarget::parse(&address) else {
            return Cmd::none();
        };
        self.picker.probed_at = Some(Instant::now());
        self.start(Action::NodeProbe {
            targets: vec![target],
        })
    }

    /// The toast, set from a test. §7a's tests drive the real key paths wherever they can; a long
    /// toast is the one thing no key produces on demand, and V15 is about what happens to one.
    #[cfg(test)]
    pub(super) fn note_for_test(&mut self, text: &str) {
        self.note(text, false);
    }

    fn note(&mut self, text: impl Into<String>, err: bool) {
        self.note = text.into();
        self.note_err = err;
        self.note_at = Instant::now();
    }

    /// §7a.7, part 1. Replace the model's `Context` from disk after anything changed
    /// settings.
    ///
    /// The defect this exists for: `Tui` held **one** `Context`, cloned at startup, and never
    /// reloaded it. `Action::NodeUseRemote` rewrote `config.toml` and the wallet service
    /// correctly — the switch really happened — but `self.ctx.settings` kept the old
    /// `node.remote`, so every poll, the Node hero, the status bar and the picker's `●` all
    /// went on naming the node the user had just left. *"When I change nodes it says I'm still
    /// on Cake Wallet."*
    ///
    /// `paths`, the `systemctl` seam and every global flag are kept: they came from argv and the
    /// environment, not from the file. A reload that fails leaves the old `Context` and says so
    /// in a toast — a TUI that exited on an unreadable settings file would be worse than one
    /// showing a stale address it tells you about.
    fn reload_ctx(&mut self) -> Result<(), Error> {
        let settings = coresettings::Settings::load(&self.ctx.paths)?;
        let timeouts = coresettings::Timeouts::from_settings(&settings.timeouts);
        self.ctx.settings = settings;
        self.ctx.timeouts = timeouts;
        self.config_mtime = std::fs::metadata(self.ctx.paths.config_file())
            .and_then(|m| m.modified())
            .ok();
        self.logs.path = self.ctx.settings.node.log_file.clone();
        Ok(())
    }

    /// §7a.7, part 2. Notice a settings change made outside this process, on the 1 s poll.
    ///
    /// Returns **false** when a reload was needed and failed, and the caller then skips the rest
    /// of the tick's outbound work: fmonero does not start checking nodes while it cannot read
    /// the settings that name them, and — the reason the return value exists at all — an action
    /// armed in the same tick would overwrite the toast this raised with its own label.
    fn maybe_reload_settings(&mut self) -> bool {
        let m = std::fs::metadata(self.ctx.paths.config_file())
            .and_then(|m| m.modified())
            .ok();
        if m == self.config_mtime {
            return true;
        }
        // Record the new mtime even when the load fails, so an unreadable file raises **one**
        // toast rather than one per second until it is fixed.
        self.config_mtime = m;
        let before = nodes::configured_address(&self.ctx);
        match self.reload_ctx() {
            Ok(()) => {
                if nodes::configured_address(&self.ctx) != before {
                    self.node_changed_under_us();
                }
                true
            }
            Err(e) => {
                self.note(format!("Could not read your settings: {e}"), true);
                false
            }
        }
    }

    /// §7a.7, part 3. The address in force has changed, so the screen catches up: the picker's
    /// `●` moves on the next render (it compares against the reloaded `Context`), the old node's
    /// cached check is dropped — it is **address-matched**, so it was never about this node — and
    /// the new one is checked at once, because leaving the pane on `Not checked yet.` for up to
    /// `node.remote_probe_sec` would contradict a row the user had just measured.
    fn node_changed_under_us(&mut self) {
        self.picker.probed_at = None;
        self.reload_picker();
    }

    fn maybe_reload_theme(&mut self) {
        let m = std::fs::metadata(theme::colors_path())
            .and_then(|m| m.modified())
            .ok();
        if m != self.theme_mtime {
            self.theme_mtime = m;
            self.theme = Theme::load();
            self.wallet.filter = restyle(&self.wallet.filter, &self.theme);
            self.logs.search = restyle(&self.logs.search, &self.theme);
            self.send.restyle(&self.theme);
        }
    }

    /// §7a.1's one implementation of "`Tab` always switches screens".
    ///
    /// Two documented exceptions, and only two: the **send confirm** modal holds a built
    /// transaction and the §2a.11 `send.lock`, and the **create-wallet** modal holds a typed
    /// password. Those are the two states where "go somewhere else" must be an explicit
    /// discard, so they consume the key and say so on their own last line. Everything else a
    /// modal holds is a selection that costs a keypress to rebuild, so it closes.
    ///
    /// A focused filter or search is **committed**, not discarded — `Enter`'s effect, not
    /// `Esc`'s — because coming back with `Shift+Tab` must find the filter the user built, and
    /// that is what makes leaving safe enough to be the default.
    fn switch_screen(&mut self, forward: bool) -> Cmd<Msg> {
        if self.send.confirming() || matches!(self.modal, Some(Modal::CreateWallet(_))) {
            self.note("Esc to discard first.", true);
            return Cmd::none();
        }
        self.show_help = false;
        if self.modal.is_some() {
            self.close_modal();
        }
        let mut commit = None;
        if self.wallet.filter_open || self.logs.search_open {
            let was_log = self.logs.search_open && self.screen == Screen::Log;
            self.wallet.filter_open = false;
            self.logs.search_open = false;
            self.wallet.filter.set_focused(false);
            self.logs.search.set_focused(false);
            if was_log {
                commit = Some(self.apply_search());
            }
        }
        let delta = if forward { 1 } else { 3 };
        let moved = self.set_screen(Screen::from_index(self.screen.index() + delta));
        match commit {
            Some(c) => Cmd::batch(vec![c, moved]),
            None => moved,
        }
    }

    fn set_screen(&mut self, s: Screen) -> Cmd<Msg> {
        self.screen = s;
        self.tabs.borrow_mut().active = s.index();
        self.wallet.detail = false;
        if s == Screen::Send {
            self.send.resume();
        } else {
            self.send.editing = false;
        }
        match s {
            Screen::Node => {
                // Entry reads the cache and probes **only the configured node**, on the §2n.4
                // schedule. A picker that measured 158 nodes because someone pressed `1` would be
                // the automatic behaviour §2n.5 rule 1 forbids.
                self.reload_picker();
                let probe = self.maybe_probe_configured();
                if self.node.extras.is_none() {
                    Cmd::batch(vec![self.load_extras(), probe])
                } else {
                    probe
                }
            }
            Screen::Wallet => {
                // A real count, so the empty state can say `0 wallets` and a user can see why `o`
                // will not help before pressing it (§7a.5).
                self.wallets = wallet_dir::list(&self.ctx).unwrap_or_default();
                if self.wallet.addresses.is_none() {
                    Cmd::batch(vec![self.load_wallet(), self.load_history()])
                } else {
                    Cmd::none()
                }
            }
            Screen::Log if self.logs.empty => self.load_log(),
            _ => Cmd::none(),
        }
    }

    /// The clipboard is `wl-copy`, as argv (§2f) — never a shell string.
    fn copy(&mut self, text: &str) {
        if proc::which("wl-copy").is_none() {
            self.note("Cannot copy: wl-copy is not installed.", true);
            return;
        }
        match proc::run_with_deadline(&["wl-copy", "--", text], Duration::from_secs(2), None) {
            Ok(o) if o.success() => self.note("Copied.", false),
            Ok(o) => self.note(format!("Could not copy: {}", o.stderr_tail()), true),
            Err(e) => self.note(e.to_string(), true),
        }
    }

    fn quit(&mut self, code: i32) -> Cmd<Msg> {
        if let Some(a) = &self.action {
            let armed = self
                .quit_armed
                .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(2));
            if !armed {
                self.quit_armed = Some((Instant::now(), code));
                let extra = if a.cancellable {
                    "Esc cancels it, or press quit again to cancel and quit"
                } else {
                    "The payment has already gone out. Press quit again to leave it running."
                };
                self.note(format!("Something is running. {extra}"), true);
                return Cmd::none();
            }
            self.action = None;
        }
        self.exit.store(code, Ordering::SeqCst);
        Cmd::quit()
    }

    // ------------------------------------------------------------ keys

    fn on_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        if k.kind == KeyEventKind::Release {
            return Cmd::none();
        }
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(Modifiers::CTRL);
        if ctrl_c {
            return self.quit(130);
        }
        // **§7a.1: `Tab` and `Shift+Tab` are resolved HERE and nowhere else.** Above the modal
        // branch, above the Send branch, above the filter/search branch, above the per-screen
        // dispatch — because "never captured" is a property only when one arm, checked first,
        // owns the key. The v5.4 code dispatched `Tab` per screen and two branches ahead of
        // those ate it (the filter's `TextInput` and `send.rs`'s `Tab | Down`), which silently
        // removed the only advertised way to change screens on a machine with no usable mouse:
        // the owner's first report, and two locally reasonable rules combining into a trap.
        if matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            return self.switch_screen(k.code == KeyCode::Tab);
        }
        if self.modal.is_some() {
            return self.on_modal_key(k);
        }
        if self.show_help {
            self.show_help = false;
            return Cmd::none();
        }
        // The Send screen owns every key while its form, its picker or its confirm modal is
        // focused — otherwise typing an address would switch screens. The confirm modal owns
        // them wherever it is drawn: a build finishes even if the user moved to another screen,
        // and a visible modal that ignored `send`/`Esc` is the worst state the money surface
        // could be in.
        if self.send.confirming() || (self.screen == Screen::Send && self.send.owns_keys()) {
            return self.on_send_key(k);
        }
        if self.wallet.detail {
            match k.code {
                KeyCode::Escape | KeyCode::Enter => {
                    self.wallet.detail = false;
                    return Cmd::none();
                }
                KeyCode::Char('n') => return self.prompt_note(),
                _ => {}
            }
        }
        if self.wallet.filter_open || self.logs.search_open {
            let done = matches!(
                k.code,
                KeyCode::Escape | KeyCode::Enter | KeyCode::Down | KeyCode::Up
            );
            if done {
                self.wallet.filter_open = false;
                self.logs.search_open = false;
                self.wallet.filter.set_focused(false);
                self.logs.search.set_focused(false);
                if self.screen == Screen::Log {
                    return self.apply_search();
                }
            } else if self.screen == Screen::Log {
                if self.logs.search.handle_event(&Event::Key(k)) {
                    return self.apply_search();
                }
            } else {
                self.wallet.filter.handle_event(&Event::Key(k));
                self.reveal();
            }
            return Cmd::none();
        }

        match k.code {
            KeyCode::Char('q') => return self.quit(self.exit.load(Ordering::SeqCst)),
            KeyCode::Char('?') => {
                self.show_help = true;
                return Cmd::none();
            }
            KeyCode::Escape => return self.cancel_action(),
            KeyCode::Char('1') => return self.set_screen(Screen::Node),
            KeyCode::Char('2') => return self.set_screen(Screen::Wallet),
            KeyCode::Char('3') => return self.set_screen(Screen::Send),
            KeyCode::Char('4') => return self.set_screen(Screen::Log),
            KeyCode::Char('r') => {
                let mut cmds = vec![self.poll()];
                match self.screen {
                    Screen::Node => cmds.push(self.load_extras()),
                    Screen::Wallet => {
                        self.wallet.err = None;
                        self.wallet.rows_err = None;
                        cmds.push(self.load_wallet());
                        cmds.push(self.load_history());
                    }
                    Screen::Log => cmds.push(self.load_log()),
                    Screen::Send => {}
                }
                return Cmd::batch(cmds);
            }
            _ => {}
        }
        match self.screen {
            Screen::Node => self.on_node_key(k),
            Screen::Wallet => self.on_wallet_key(k),
            Screen::Log => self.on_log_key(k),
            Screen::Send => {
                // The form is unfocused: Enter returns to it. `Tab` never reaches here (§7a.1).
                if k.code == KeyCode::Enter {
                    self.send.resume();
                }
                Cmd::none()
            }
        }
    }

    // ------------------------------------------------------------ mouse (§7a.2)

    /// One entry point, resolving through [`hit::Map::at`]'s priority chain.
    ///
    /// **Semantics chosen so nothing needs a timer**: left down selects (a row, a field, a tab) or
    /// activates (a button); left down on an **already-selected** row activates it, which is
    /// `TableState::handle_mouse`'s own rule and replaces double-click detection entirely — there
    /// is no interval to get wrong on a slow terminal, and a real double click produces exactly
    /// that sequence. Right click is `Esc` for the current context, one meaning everywhere.
    /// Middle click, `Drag`, `Moved` and horizontal scroll are ignored, and **no hover state is
    /// ever set**: a highlight a keyboard cannot produce is a difference between the two input
    /// methods, and §7a's premise is that they are peers.
    fn on_mouse(&mut self, ev: MouseEvent) -> Cmd<Msg> {
        use MouseEventKind as K;
        let scroll = match ev.kind {
            K::ScrollUp => Some(true),
            K::ScrollDown => Some(false),
            K::Down(MouseButton::Right) => return self.mouse_escape(),
            K::Down(MouseButton::Left) => None,
            // Drag, Moved, Up, Middle and horizontal scroll: ignored, deliberately.
            _ => return Cmd::none(),
        };
        let Some(target) = self.hits.at(ev.x, ev.y, scroll) else {
            // A click that hits no target is discarded **silently** — no beep, no toast, no focus
            // change. A click that does something arbitrary is worse than one that does nothing.
            return Cmd::none();
        };
        // **Money is untouched by the mouse**: while a relay is in flight the only reachable
        // target is the status line's own help (§7a.2).
        if self.action.as_ref().is_some_and(|a| !a.cancellable)
            && target != hit::Target::Button(hit::Button::Help)
        {
            return Cmd::none();
        }
        match target {
            hit::Target::Tab(i) => self.set_screen(Screen::from_index(i)),
            hit::Target::Scroll { pane, up } => {
                self.scroll_pane(pane, up);
                Cmd::none()
            }
            hit::Target::Row { pane, row } => self.click_row(pane, row),
            hit::Target::Field(i) => self.click_field(i),
            hit::Target::Priority(level) => {
                self.send.priority = level;
                Cmd::none()
            }
            hit::Target::Arrow { which, delta } => self.click_arrow(which, delta),
            hit::Target::Button(b) => self.click_button(b),
            hit::Target::ModalButton(_) => {
                // Every modal fmonero draws has one primary button, and it does what `Enter`
                // does — the key path and the click path are the same code (§7a.2).
                self.on_modal_key(KeyEvent::new(KeyCode::Enter))
            }
            hit::Target::Backdrop => self.mouse_escape(),
        }
    }

    /// Right click, and a backdrop click: `Esc` for the current context — close the overlay, else
    /// close the modal, else unfocus a field, else cancel the running action.
    ///
    /// **The two modals §7a.1 protects are protected here too**: a misplaced click must never
    /// discard a built transaction or a half-typed password.
    fn mouse_escape(&mut self) -> Cmd<Msg> {
        if self.show_help {
            self.show_help = false;
            return Cmd::none();
        }
        if self.send.confirming() || matches!(self.modal, Some(Modal::CreateWallet(_))) {
            self.note("Esc to discard first.", true);
            return Cmd::none();
        }
        if self.modal.is_some() {
            self.close_modal();
            return Cmd::none();
        }
        if self.wallet.filter_open || self.logs.search_open || self.send.editing {
            self.wallet.filter_open = false;
            self.logs.search_open = false;
            self.wallet.filter.set_focused(false);
            self.logs.search.set_focused(false);
            self.send.editing = false;
            return Cmd::none();
        }
        self.cancel_action()
    }

    fn scroll_pane(&mut self, pane: hit::HitPane, up: bool) {
        let rows = 3;
        match pane {
            hit::HitPane::Picker => {
                let n = self.picker_rows().len();
                let mut st = self.picker.table.borrow_mut();
                if up {
                    st.scroll_up(rows);
                } else {
                    st.scroll_down(rows, n);
                }
            }
            hit::HitPane::NodeList => {
                let mut st = self.node.table.borrow_mut();
                if up {
                    st.scroll_up(rows);
                } else {
                    st.scroll_down(rows, usize::MAX / 2);
                }
            }
            hit::HitPane::Subaddresses => {
                let mut st = self.wallet.table.borrow_mut();
                if up {
                    st.scroll_up(rows);
                } else {
                    st.scroll_down(rows, usize::MAX / 2);
                }
            }
            hit::HitPane::History => {
                let n = self.wallet.rows.len() as i32;
                let delta = if up { -(rows as i32) } else { rows as i32 };
                self.wallet
                    .list
                    .borrow_mut()
                    .scroll(delta, n.max(0) as usize);
            }
            hit::HitPane::Log => {
                let mut v = self.logs.viewer.borrow_mut();
                if up {
                    v.scroll_up(rows);
                } else {
                    v.scroll_down(rows);
                }
            }
            hit::HitPane::ModalList => {}
        }
    }

    /// A click on a row: select it, or — when it is already selected — activate it, which is what
    /// `Enter` does on that pane.
    fn click_row(&mut self, pane: hit::HitPane, row: usize) -> Cmd<Msg> {
        match pane {
            hit::HitPane::Picker => {
                let n = self.picker_rows().len();
                if row >= n {
                    return Cmd::none();
                }
                let already = self.picker.table.borrow().selected == Some(row);
                self.picker.table.borrow_mut().select(Some(row));
                if already {
                    return self.use_selected_node();
                }
                Cmd::none()
            }
            hit::HitPane::Subaddresses => {
                let already = self.wallet.table.borrow().selected == Some(row);
                self.wallet.table.borrow_mut().select(Some(row));
                self.wallet.pane = Pane::Subaddresses;
                if already {
                    if let Some(a) = self.selected_address() {
                        self.copy(&a);
                    }
                }
                Cmd::none()
            }
            hit::HitPane::History => {
                let already = self.wallet.list.borrow().selected == Some(row);
                self.wallet.list.borrow_mut().selected = Some(row);
                self.wallet.pane = Pane::History;
                if already {
                    self.wallet.detail = self.selected_transfer().is_some();
                }
                Cmd::none()
            }
            hit::HitPane::NodeList => {
                self.node.table.borrow_mut().select(Some(row));
                Cmd::none()
            }
            hit::HitPane::Log | hit::HitPane::ModalList => Cmd::none(),
        }
    }

    /// A click on an input: focus it. On the Send screen that is a form field; inside a modal it
    /// is that modal's nth control.
    fn click_field(&mut self, index: usize) -> Cmd<Msg> {
        match &mut self.modal {
            Some(Modal::CreateWallet(form)) => {
                form.field = index.min(3);
                form.name.set_focused(index == 0);
                form.pass.set_focused(index == 1);
                form.again.set_focused(index == 2);
            }
            Some(Modal::AddNode { input }) => input.set_focused(true),
            _ => {
                self.send.focus_index(index);
                self.screen = Screen::Send;
                self.tabs.borrow_mut().active = Screen::Send.index();
            }
        }
        Cmd::none()
    }

    fn click_arrow(&mut self, which: hit::Selector, delta: i32) -> Cmd<Msg> {
        match which {
            hit::Selector::Language => {
                if let Some(Modal::CreateWallet(form)) = &mut self.modal {
                    let n = coresettings::SEED_LANGUAGES.len();
                    form.lang = if delta < 0 {
                        (form.lang + n - 1) % n
                    } else {
                        (form.lang + 1) % n
                    };
                }
            }
            hit::Selector::Account => {
                let n = self
                    .status
                    .as_ref()
                    .and_then(|s| s.wallet.accounts)
                    .unwrap_or(1)
                    .max(1);
                self.send.account = if delta < 0 {
                    (self.send.account + n - 1) % n
                } else {
                    (self.send.account + 1) % n
                };
            }
        }
        Cmd::none()
    }

    /// A rendered chip does exactly what its key does — **one** code path, which is what makes
    /// "two affordances, one label" true rather than a promise the two halves make separately.
    fn click_button(&mut self, b: hit::Button) -> Cmd<Msg> {
        match b {
            hit::Button::Help => {
                self.show_help = !self.show_help;
                Cmd::none()
            }
            hit::Button::NextScreen => self.switch_screen(true),
            hit::Button::OpenWallet => self.open_wallet_modal(),
            hit::Button::CreateWallet => self.create_wallet_modal(),
            hit::Button::CloseWallet => self.close_wallet(),
            hit::Button::WalletDaemon => self.toggle_wallet_daemon(),
            hit::Button::RefreshWallet => self.start(Action::WalletRefresh { start_height: 0 }),
            hit::Button::Qr => {
                match self.selected_address() {
                    Some(address) => self.modal = Some(Modal::Qr { address }),
                    None => self.note("No address to show yet.", true),
                }
                Cmd::none()
            }
            hit::Button::NodeUp => self.confirm(Confirm::NodeUp),
            hit::Button::NodeDown => self.confirm(Confirm::NodeDown),
            hit::Button::UseLocal => self.use_local(),
            hit::Button::Probe => self.start_probe(),
            hit::Button::RefreshList => self.start_fetch(true),
            hit::Button::AddNode => self.add_node_modal(),
            hit::Button::UseNode => self.use_selected_node(),
            hit::Button::PrevNode => self.previous_node(),
        }
    }

    fn on_node_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        // The picker's filter is a focused input: it keeps every printable character and loses
        // only `Tab`, which §7a.1 resolved above this function.
        if self.picker.filter_open {
            match k.code {
                KeyCode::Escape | KeyCode::Enter | KeyCode::Down | KeyCode::Up => {
                    self.picker.filter_open = false;
                    self.picker.filter.set_focused(false);
                }
                _ => {
                    self.picker.filter.handle_event(&Event::Key(k));
                    // A filter that moved cannot keep a selection made against the old rows.
                    self.picker.table.borrow_mut().select(None);
                }
            }
            return Cmd::none();
        }
        match k.code {
            // ---- the public-node picker (§7a.4) ----------------------------------
            KeyCode::Char('p') => return self.start_probe(),
            KeyCode::Char('f') => return self.start_fetch(true),
            KeyCode::Enter => return self.use_selected_node(),
            KeyCode::Char('a') => return self.add_node_modal(),
            KeyCode::Char('H') => return self.previous_node(),
            KeyCode::Char('/') => {
                self.picker.filter_open = true;
                self.picker.filter.set_focused(true);
                return Cmd::none();
            }
            KeyCode::Char('u') => return self.confirm(Confirm::NodeUp),
            KeyCode::Char('d') => return self.confirm(Confirm::NodeDown),
            // v5.6, §2o.1. The way out of public-node mode, from inside the app. It bypasses
            // `confirm`'s remote-mode guard deliberately: that guard is what this key answers.
            KeyCode::Char('l') => return self.use_local(),
            KeyCode::Char('L') => {
                let next = (self.logs.level + 1) % 5;
                return self.confirm(Confirm::LogLevel(next));
            }
            KeyCode::Char('b') => {
                self.node.view = match self.node.view {
                    NodeView::Peers => NodeView::Bans,
                    NodeView::Bans => NodeView::Peers,
                };
                return self.load_extras();
            }
            // `↑↓` move the **picker** on this screen: it is the pane a user acts on, and the
            // peers table is a read-only display whose selection nothing consumes.
            KeyCode::Down | KeyCode::Char('j') => self.move_picker(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_picker(-1),
            _ => {}
        }
        Cmd::none()
    }

    /// `↑↓` on the picker. From **no** selection the first press selects row 0, which is how a
    /// keyboard user reaches the table that deliberately opens with nothing selected (revision 4).
    fn move_picker(&mut self, delta: i32) {
        let n = self.picker_rows().len();
        if n == 0 {
            return;
        }
        let mut st = self.picker.table.borrow_mut();
        let next = match st.selected {
            None => 0,
            Some(i) => (i as i32 + delta).clamp(0, n as i32 - 1) as usize,
        };
        st.select(Some(next));
    }

    fn on_wallet_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            // `←→` swaps the Wallet's panes. `Tab` never reaches here: §7a.1 resolves it in
            // `on_key`'s first arm, on every screen and in every focus state.
            KeyCode::Left | KeyCode::Right => {
                self.wallet.pane = match self.wallet.pane {
                    Pane::Subaddresses => Pane::History,
                    Pane::History => Pane::Subaddresses,
                };
            }
            KeyCode::Char('o') => return self.open_wallet_modal(),
            // **`c` is create and `x` closes** (§7a.5). The old binding was the collision that
            // made the one key that looks like "create" do nothing visible on a machine with no
            // wallets — half of the owner's complaint (4).
            KeyCode::Char('c') => return self.create_wallet_modal(),
            KeyCode::Char('x') => return self.close_wallet(),
            KeyCode::Char('W') => return self.toggle_wallet_daemon(),
            KeyCode::Char('a') => {
                let account = self.account_index();
                return self.task("new address", move |ctx| {
                    let w = WalletClient::from_ctx(ctx)?;
                    let a = w.create_address(account, "")?;
                    Ok(format!("new address {}", a.address_index))
                });
            }
            KeyCode::Char('l') => return self.prompt_label(),
            KeyCode::Char('n') => return self.prompt_note(),
            KeyCode::Char('R') => {
                return self.start(Action::WalletRefresh { start_height: 0 });
            }
            KeyCode::Char('Q') => match self.selected_address() {
                Some(address) => self.modal = Some(Modal::Qr { address }),
                None => self.note("No address to show yet.", true),
            },
            KeyCode::Char('/') => {
                self.wallet.filter_open = true;
                self.wallet.filter.set_focused(true);
                self.wallet.pane = Pane::History;
            }
            KeyCode::Char('t') => {
                self.wallet.kind = match self.wallet.kind {
                    None => Some(TransferKind::In),
                    Some(TransferKind::In) => Some(TransferKind::Out),
                    Some(TransferKind::Out) => Some(TransferKind::Block),
                    Some(TransferKind::Block) => Some(TransferKind::Pending),
                    Some(TransferKind::Pending) => Some(TransferKind::Failed),
                    Some(TransferKind::Failed) => Some(TransferKind::Pool),
                    Some(TransferKind::Pool) => None,
                };
                self.wallet.list.borrow_mut().selected = Some(0);
                return self.load_history();
            }
            KeyCode::Enter => match self.wallet.pane {
                Pane::Subaddresses => {
                    if let Some(a) = self.selected_address() {
                        self.copy(&a);
                    }
                }
                Pane::History => self.wallet.detail = self.selected_transfer().is_some(),
            },
            KeyCode::Down | KeyCode::Char('j') => self.move_list(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_list(-1),
            KeyCode::PageDown => self.move_list(10),
            KeyCode::PageUp => self.move_list(-10),
            _ => {}
        }
        Cmd::none()
    }

    fn on_log_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Char('/') => {
                self.logs.search_open = true;
                self.logs.search.set_focused(true);
            }
            KeyCode::Char('f') => {
                self.logs.follow = !self.logs.follow;
                self.logs
                    .viewer
                    .borrow_mut()
                    .set_auto_scroll(self.logs.follow);
                let on = self.logs.follow;
                self.note(
                    if on {
                        "Following the log."
                    } else {
                        "Not following the log."
                    },
                    false,
                );
            }
            KeyCode::Char('L') => {
                let next = (self.logs.level + 1) % 5;
                return self.confirm(Confirm::LogLevel(next));
            }
            KeyCode::Char('n') => self.logs.viewer.borrow_mut().next_match(),
            KeyCode::Char('N') => self.logs.viewer.borrow_mut().prev_match(),
            KeyCode::Down | KeyCode::Char('j') => self.logs.viewer.borrow_mut().scroll_down(1),
            KeyCode::Up | KeyCode::Char('k') => self.logs.viewer.borrow_mut().scroll_up(1),
            KeyCode::PageDown => self.logs.viewer.borrow_mut().scroll_down(20),
            KeyCode::PageUp => self.logs.viewer.borrow_mut().scroll_up(20),
            KeyCode::Home => self.logs.viewer.borrow_mut().scroll_to_top(),
            KeyCode::End => self.logs.viewer.borrow_mut().scroll_to_bottom(),
            _ => {}
        }
        Cmd::none()
    }

    fn on_send_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        // How many accounts the wallet has, so the selector cannot name a missing one (§5).
        let accounts = self
            .status
            .as_ref()
            .and_then(|s| s.wallet.accounts)
            .unwrap_or(1);
        match self.send.on_key(k, accounts) {
            SendCmd::None => Cmd::none(),
            SendCmd::Note(text) => {
                self.note(text, true);
                Cmd::none()
            }
            SendCmd::Info(text) => {
                self.note(text, false);
                Cmd::none()
            }
            SendCmd::Book => self.load_book(),
            SendCmd::Validate => match self.send.request(None) {
                Ok(req) => self.validate_then_build(req),
                Err(why) => {
                    self.send.back_to_form();
                    self.note(why, true);
                    Cmd::none()
                }
            },
            SendCmd::Relay => {
                let Stage::Built(plan) = std::mem::replace(&mut self.send.stage, Stage::Relaying)
                else {
                    return Cmd::none();
                };
                self.send.typed.clear();
                // The SAME plan that was displayed (§2a.3): it is moved, never rebuilt. And if
                // `try_start` refuses, the plan goes back into `Built` rather than being dropped
                // with the screen stuck on "broadcasting" for the rest of the session.
                match self.try_start(Action::Relay(*plan)) {
                    Ok(cmd) => cmd,
                    Err(refused) => {
                        match *refused {
                            Action::Relay(plan) => self.send.stage = Stage::Built(Box::new(plan)),
                            _ => self.send.back_to_form(),
                        }
                        Cmd::none()
                    }
                }
            }
        }
    }

    /// The modal is taken out of the model for the duration of the key press and put back
    /// unless it closed — so a handler can call `self.task(…)` and `self.start(…)` without
    /// holding a borrow of the field it is about to replace.
    fn on_modal_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        let Some(modal) = self.modal.take() else {
            return Cmd::none();
        };
        match modal {
            Modal::Qr { .. } => Cmd::none(),
            Modal::Confirm { what, .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => match what {
                    Confirm::NodeUp => self.start(Action::NodeUp),
                    Confirm::NodeDown => self.start(Action::NodeDown),
                    Confirm::NodeLocal => {
                        self.picker.prev = fmonero_core::nodes::configured_address(&self.ctx);
                        self.start(Action::NodeLocal)
                    }
                    Confirm::LogLevel(level) => {
                        // The daemon does not report its level, so the cycle pointer is the
                        // TUI's own and advances when the call is sent, not when it lands.
                        self.logs.level = level;
                        self.task("log level", move |ctx| {
                            let n = NodeClient::from_ctx(ctx)?;
                            n.mutators(ctx)?.set_log_level(level)?;
                            Ok(format!("your node now logs at level {level}"))
                        })
                    }
                },
                _ => {
                    self.note("Cancelled.", false);
                    Cmd::none()
                }
            },
            Modal::Prompt {
                what,
                title,
                mut input,
            } => match k.code {
                KeyCode::Escape => Cmd::none(),
                KeyCode::Enter => {
                    let text = input.value().to_string();
                    match what {
                        Prompt::Label { index } => {
                            let account = self.account_index();
                            self.task("label", move |ctx| {
                                let w = WalletClient::from_ctx(ctx)?;
                                w.label_address(account, index, &text)?;
                                Ok("Name saved.".into())
                            })
                        }
                        Prompt::Note { txid } => self.task("note", move |ctx| {
                            let w = WalletClient::from_ctx(ctx)?;
                            w.set_tx_notes(&[txid], &[text])?;
                            Ok("Note saved.".into())
                        }),
                    }
                }
                _ => {
                    input.handle_event(&Event::Key(k));
                    self.modal = Some(Modal::Prompt { what, title, input });
                    Cmd::none()
                }
            },
            Modal::OpenWallet {
                names,
                mut sel,
                mut pass,
                mut asking,
            } => {
                if asking {
                    match k.code {
                        KeyCode::Escape => {
                            zero(&mut pass);
                            return Cmd::none();
                        }
                        KeyCode::Enter => {
                            let name = names.get(sel).cloned().unwrap_or_default();
                            let password = pass.value().to_string();
                            zero(&mut pass);
                            return self.start(Action::WalletOpen { name, password });
                        }
                        _ => {
                            pass.handle_event(&Event::Key(k));
                        }
                    }
                } else {
                    match k.code {
                        KeyCode::Escape => return Cmd::none(),
                        KeyCode::Down | KeyCode::Char('j') => {
                            sel = (sel + 1).min(names.len().saturating_sub(1));
                        }
                        KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                        KeyCode::Enter if !names.is_empty() => {
                            asking = true;
                            pass.set_focused(true);
                        }
                        _ => {}
                    }
                }
                self.modal = Some(Modal::OpenWallet {
                    names,
                    sel,
                    pass,
                    asking,
                });
                Cmd::none()
            }
            // ---- v5.5 --------------------------------------------------------------
            Modal::SeedBackup { lines } => match k.code {
                KeyCode::Enter | KeyCode::Escape => Cmd::none(),
                _ => {
                    self.modal = Some(Modal::SeedBackup { lines });
                    Cmd::none()
                }
            },
            Modal::CreateWallet(form) => {
                let CreateForm {
                    mut name,
                    mut pass,
                    mut again,
                    mut lang,
                    mut field,
                    err,
                } = *form;
                // §7a.1, revision 1: `Up`/`Down` move, `Enter` advances from every control but
                // the last where it submits, `Esc` unfocuses and then discards. This modal
                // consumes `Tab` (it holds a typed password), so without a field key the flow
                // complaint (4) asked for would be unreachable from a keyboard.
                const LAST: usize = 3;
                let refocus = |name: &mut TextInput,
                               pass: &mut TextInput,
                               again: &mut TextInput,
                               field: usize| {
                    name.set_focused(field == 0);
                    pass.set_focused(field == 1);
                    again.set_focused(field == 2);
                };
                match k.code {
                    KeyCode::Escape => {
                        zero(&mut pass);
                        zero(&mut again);
                        return Cmd::none();
                    }
                    KeyCode::Down => field = (field + 1).min(LAST),
                    KeyCode::Up => field = field.saturating_sub(1),
                    KeyCode::Enter if field < LAST => field += 1,
                    KeyCode::Enter => {
                        let want = name.value().trim().to_string();
                        let p1 = pass.value().to_string();
                        let p2 = again.value().to_string();
                        let language = coresettings::SEED_LANGUAGES[lang].1.to_string();
                        match self.create_wallet_refusal(&want, &p1, &p2) {
                            Some(why) => {
                                // Refused inline; nothing is sent, and the typed password stays
                                // so the user can fix the other field.
                                self.modal = Some(Modal::CreateWallet(Box::new(CreateForm {
                                    name,
                                    pass,
                                    again,
                                    lang,
                                    field,
                                    err: Some(why),
                                })));
                                return Cmd::none();
                            }
                            None => {
                                zero(&mut again);
                                zero(&mut pass);
                                return self.start(Action::WalletCreate {
                                    name: want,
                                    password: p1,
                                    language,
                                });
                            }
                        }
                    }
                    KeyCode::Left if field == LAST => {
                        lang = (lang + coresettings::SEED_LANGUAGES.len() - 1)
                            % coresettings::SEED_LANGUAGES.len();
                    }
                    KeyCode::Right if field == LAST => {
                        lang = (lang + 1) % coresettings::SEED_LANGUAGES.len();
                    }
                    _ => {
                        match field {
                            0 => name.handle_event(&Event::Key(k)),
                            1 => pass.handle_event(&Event::Key(k)),
                            2 => again.handle_event(&Event::Key(k)),
                            _ => false,
                        };
                    }
                }
                refocus(&mut name, &mut pass, &mut again, field);
                self.modal = Some(Modal::CreateWallet(Box::new(CreateForm {
                    name,
                    pass,
                    again,
                    lang,
                    field,
                    err,
                })));
                Cmd::none()
            }
            Modal::UseNode {
                address,
                wallet_open,
            } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.picker.prev = fmonero_core::nodes::configured_address(&self.ctx);
                    self.start(Action::NodeUseRemote {
                        address,
                        trusted: false,
                    })
                }
                KeyCode::Escape => {
                    self.note("Cancelled. The node was not changed.", false);
                    Cmd::none()
                }
                _ => {
                    self.modal = Some(Modal::UseNode {
                        address,
                        wallet_open,
                    });
                    Cmd::none()
                }
            },
            Modal::AddNode { mut input } => match k.code {
                KeyCode::Escape => Cmd::none(),
                KeyCode::Enter => {
                    let typed = input.value().trim().to_string();
                    match fmonero_core::nodes::add_custom(&self.ctx, &typed) {
                        Ok(node) => {
                            self.note(format!("Added {}.", node.address), false);
                            self.reload_picker();
                        }
                        // The SAME function the CLI validates with, so the TUI cannot accept a
                        // string the CLI refuses — and the message is §2m.2's own (§5b).
                        Err(e) => {
                            self.modal = Some(Modal::AddNode { input });
                            self.note(e.to_string(), true);
                        }
                    }
                    Cmd::none()
                }
                _ => {
                    input.handle_event(&Event::Key(k));
                    self.modal = Some(Modal::AddNode { input });
                    Cmd::none()
                }
            },
            Modal::Notice { then } => match k.code {
                KeyCode::Enter => {
                    // `Enter` is the consent record, written to the cache's `notice_ack` — not
                    // the cache's existence, which a bare `node list` can create while printing
                    // nothing (§2n.5 rule 2, revision 7).
                    if let Err(e) = fmonero_core::nodes::ack_notice(&self.ctx) {
                        self.note(e.to_string(), true);
                        return Cmd::none();
                    }
                    match then {
                        NoticeThen::Fetch => self.start_fetch(true),
                        NoticeThen::Probe => self.start_probe(),
                    }
                }
                KeyCode::Escape => {
                    self.note("Nothing was contacted.", false);
                    Cmd::none()
                }
                _ => {
                    self.modal = Some(Modal::Notice { then });
                    Cmd::none()
                }
            },
        }
    }

    /// Close whatever modal is open, zeroing every password buffer it held (§2b.4). Used by
    /// §7a.1's `Tab` (which closes every modal but the two that hold something undroppable) and
    /// by a backdrop click (§7a.2).
    fn close_modal(&mut self) {
        match self.modal.take() {
            Some(Modal::OpenWallet { mut pass, .. }) => zero(&mut pass),
            Some(Modal::CreateWallet(mut form)) => {
                zero(&mut form.pass);
                zero(&mut form.again);
            }
            _ => {}
        }
    }

    /// Every local refusal §7a.5 names, in the order a user meets them, so nothing is sent for
    /// a mistake fmonero can see: a `/` in the name (§1b.5), a wallet that already exists, a
    /// mismatched pair, and an empty password — for which there is deliberately **no** TUI
    /// equivalent of `--allow-empty-password` (§4.1: an unencrypted wallet is a choice, and the
    /// deliberate place to make it is a command line).
    fn create_wallet_refusal(&self, name: &str, p1: &str, p2: &str) -> Option<String> {
        if name.is_empty() {
            return Some("a wallet name is required".into());
        }
        if name.contains('/') {
            return Some("A wallet name cannot contain \"/\".".into());
        }
        if self
            .ctx
            .settings
            .wallet
            .dir
            .join(format!("{name}.keys"))
            .exists()
        {
            return Some(format!("{name} already exists in wallet.dir"));
        }
        if p1 != p2 {
            return Some("the two passwords do not match".into());
        }
        if p1.is_empty() {
            return Some(
                "an empty password would leave the wallet unencrypted — run: \
                 fmonero wallet create <name> --allow-empty-password --yes"
                    .into(),
            );
        }
        None
    }

    // ------------------------------------------------------------ modal builders

    /// `Confirm::NodeLocal`'s own entry point (v5.6). It is **not** `confirm`, because
    /// `confirm`'s first act is to refuse in public-node mode and this is the one confirm that
    /// only exists there.
    fn confirm_local(&mut self) -> Cmd<Msg> {
        let mut state = DialogState::new();
        state.focused_button = Some(0);
        // **V4: the words are in `screens::local_node_modal`**, which draws them as the overlay
        // §7a.4 specifies — three sentences, and `Dialog` renders one centred row. The `Dialog`
        // here is what the key handler and the mouse's `ModalButton` share with every other
        // confirm; its message is never drawn.
        self.modal = Some(Modal::Confirm {
            what: Confirm::NodeLocal,
            dialog: Box::new(screens::confirm_dialog(
                &self.theme,
                " Use your own node ",
                "",
            )),
            state: RefCell::new(state),
        });
        Cmd::none()
    }

    fn confirm(&mut self, what: Confirm) -> Cmd<Msg> {
        // In public-node mode `u`, `d` and `L` say so instead of acting (§7a): all three exit 4
        // `remote_node`. **v5.6 names the two keys that do something about it** rather than the
        // shell command the refusal used to print (§2o.1) — `f` to pick another public node, `l`
        // to go back to your own. The switch still asks first: it restarts the wallet service and
        // closes the wallet this session is holding (§2m.5).
        if self.remote_mode() {
            // **W14: short enough to be read whole.** The status bar has ~37 columns left for a
            // toast at 120, and this sentence was cut to `You are using a public node. Press…` at
            // both widths — so the two in-app routes it exists to name were never on screen. The
            // situation is on the Node screen in front of the user; what the toast owes them is
            // the two keys.
            // 40 columns, measured against the narrowest room the bar has for a toast: the
            // left segment carries two badges and the node's address, which leaves 44 columns
            // at 120 with a 22-character address, and every column over that is a cut sentence.
            self.note("Press f for the list, or l for your own.", true);
            return Cmd::none();
        }
        if !matches!(units::may_mutate_node(&self.ctx), Ok(true)) {
            self.note(fmonero_core::FOREIGN_NODE_READ_ONLY, true);
            return Cmd::none();
        }
        // **W2: `NodeUp` and `NodeDown` are overlays now**, like `NodeLocal`, and their words are
        // in `screens::node_power_modal`. The `Dialog` built here is what the key handler and the
        // mouse's `ModalButton` share with every confirm; its message is never drawn for these
        // three, because `Dialog` renders a message as one centred row and each of them has
        // several sentences to state (V4).
        let (title, message) = match what {
            Confirm::NodeUp => (" Start your node ", ""),
            Confirm::NodeDown => (" Stop your node ", ""),
            Confirm::NodeLocal => (" Use your own node ", ""),
            // One row is enough here — it has one fact to state — but it states the same two
            // keys as every other confirm (self-read, round 3: it named `y` and not `Esc`).
            Confirm::LogLevel(l) => (
                " How much your node logs ",
                match l {
                    0 => "Set it to 0, the quietest. y to confirm · Esc cancels",
                    1 => "Set it to 1, where 0 is quietest. y to confirm · Esc cancels",
                    2 => "Set it to 2, where 0 is quietest. y to confirm · Esc cancels",
                    3 => "Set it to 3, where 0 is quietest. y to confirm · Esc cancels",
                    _ => "Set it to 4, the noisiest. y to confirm · Esc cancels",
                },
            ),
        };
        let mut state = DialogState::new();
        state.focused_button = Some(0);
        self.modal = Some(Modal::Confirm {
            what,
            dialog: Box::new(screens::confirm_dialog(&self.theme, title, message)),
            state: RefCell::new(state),
        });
        Cmd::none()
    }

    fn open_wallet_modal(&mut self) -> Cmd<Msg> {
        let names = wallet_dir::list(&self.ctx).unwrap_or_default();
        let mut pass = TextInput::new()
            .with_mask('•')
            .with_style(self.theme.text())
            .with_cursor_style(Style::new().bg(self.theme.accent).fg(self.theme.selection));
        pass.set_focused(false);
        self.modal = Some(Modal::OpenWallet {
            names,
            sel: 0,
            pass,
            asking: false,
        });
        Cmd::none()
    }

    /// §7a.5's create-wallet modal. `c` is **create** now, and `x` closes: `c` for "close"
    /// against `c` for "create" was a collision the old keymap resolved the wrong way — closing a
    /// wallet is rare, reversible and reachable from a screen showing an open wallet, while
    /// creating one is the thing a new user needs and the thing `c` universally suggests.
    fn create_wallet_modal(&mut self) -> Cmd<Msg> {
        let masked = |th: &Theme| {
            TextInput::new()
                .with_mask('•')
                .with_style(th.text())
                .with_cursor_style(Style::new().bg(th.accent).fg(th.selection))
        };
        let mut name = text_input(&self.theme);
        name.set_focused(true);
        // `English` is the default, and it is index 1 of `SEED_LANGUAGES` (upstream's order).
        let lang = coresettings::SEED_LANGUAGES
            .iter()
            .position(|(_, en)| *en == coresettings::DEFAULT_SEED_LANGUAGE)
            .unwrap_or(0);
        self.modal = Some(Modal::CreateWallet(Box::new(CreateForm {
            name,
            pass: masked(&self.theme),
            again: masked(&self.theme),
            lang,
            field: 0,
            err: None,
        })));
        Cmd::none()
    }

    fn add_node_modal(&mut self) -> Cmd<Msg> {
        let mut input = text_input(&self.theme);
        input.set_focused(true);
        self.modal = Some(Modal::AddNode { input });
        Cmd::none()
    }

    /// `x` — close the open wallet.
    fn close_wallet(&mut self) -> Cmd<Msg> {
        self.task("close", |ctx| {
            let w = WalletClient::from_ctx(ctx)?;
            fmonero_core::wallet_session::close(ctx, &w)?;
            Ok("Wallet closed.".into())
        })
    }

    /// Does `W` stop the wallet service right now, rather than start it (W15)?
    ///
    /// **The chip's label reads this**, so `[W] Start the wallet service` can no longer sit on a
    /// screen where pressing `W` stops it — which is what the empty state did in exactly the state
    /// §7a.5 describes. One predicate, and it is the one [`Tui::toggle_wallet_daemon`] branches on.
    pub(super) fn wallet_daemon_key_stops(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.wallet.unit_active && !s.wallet.open && s.wallet.name.is_none())
    }

    /// `W` — start the wallet daemon, or stop it when it is idle.
    ///
    /// **The same key stops it, and only when the unit is active, no wallet is open and nothing is
    /// in flight** (§7a.5): stopping a daemon holding an open wallet is a thing the user will not
    /// have meant, and `x` then `W` is the explicit two-step.
    fn toggle_wallet_daemon(&mut self) -> Cmd<Msg> {
        let Some(s) = self.status.as_ref() else {
            return self.start(Action::WalletDaemonStart);
        };
        if !s.wallet.unit_active {
            return self.start(Action::WalletDaemonStart);
        }
        if s.wallet.open || s.wallet.name.is_some() {
            self.note("A wallet is open. Press x to close it first.", true);
            return Cmd::none();
        }
        if self.action.is_some() {
            self.note("Something is already running.", true);
            return Cmd::none();
        }
        self.start(Action::WalletDaemonStop)
    }

    fn prompt_label(&mut self) -> Cmd<Msg> {
        let Some(set) = &self.wallet.addresses else {
            self.note("No address selected.", true);
            return Cmd::none();
        };
        let i = self.wallet.table.borrow().selected.unwrap_or(0);
        let Some(row) = set.addresses.get(i) else {
            self.note("No address selected.", true);
            return Cmd::none();
        };
        self.modal = Some(Modal::Prompt {
            what: Prompt::Label {
                index: row.address_index,
            },
            title: format!(" Name address {} ", row.address_index),
            input: text_input(&self.theme)
                .with_value(row.label.clone())
                .with_focused(true),
        });
        Cmd::none()
    }

    fn prompt_note(&mut self) -> Cmd<Msg> {
        let Some(t) = self.selected_transfer() else {
            self.note("No payment selected.", true);
            return Cmd::none();
        };
        let txid = t.txid.clone();
        let value = t.note.clone();
        self.modal = Some(Modal::Prompt {
            what: Prompt::Note { txid },
            title: " Note for this transfer ".to_string(),
            input: text_input(&self.theme).with_value(value).with_focused(true),
        });
        Cmd::none()
    }

    // ------------------------------------------------------------ list movement

    fn move_list(&mut self, delta: isize) {
        if self.wallet.pane == Pane::Subaddresses {
            self.move_table(delta);
            return;
        }
        let total = self.visible_history().len();
        if total == 0 {
            return;
        }
        {
            let mut st = self.wallet.list.borrow_mut();
            let cur = st.selected.unwrap_or(0) as isize;
            st.selected = Some((cur + delta).clamp(0, total as isize - 1) as usize);
        }
        self.reveal();
    }

    fn move_table(&mut self, delta: isize) {
        let total = match self.screen {
            Screen::Wallet => self
                .wallet
                .addresses
                .as_ref()
                .map_or(0, |a| a.addresses.len()),
            _ => self
                .node
                .extras
                .as_ref()
                .map_or(0, |e| e.connections.len().min(10)),
        };
        if total == 0 {
            return;
        }
        let cell = match self.screen {
            Screen::Wallet => &self.wallet.table,
            _ => &self.node.table,
        };
        let mut st = cell.borrow_mut();
        let cur = st.selected.unwrap_or(0) as isize;
        st.selected = Some((cur + delta).clamp(0, total as isize - 1) as usize);
    }

    /// Keep the highlighted row inside the viewport without snapping it to the top.
    fn reveal(&self) {
        let total = self.visible_history().len();
        let mut st = self.wallet.list.borrow_mut();
        let Some(sel) = st.selected else { return };
        let vis = st.visible_count().max(1);
        let off = st.scroll_offset_clamped(total);
        if sel < off {
            st.scroll_to(sel, total);
        } else if sel + 1 > off + vis {
            st.scroll_to(sel + 1 - vis, total);
        }
    }

    fn apply_search(&mut self) -> Cmd<Msg> {
        let q = self.logs.search.value().trim().to_string();
        let mut log = self.logs.viewer.borrow_mut();
        if q.is_empty() {
            log.clear_search();
        } else {
            log.search(&q);
        }
        Cmd::none()
    }

    /// Move whatever `send::relay` announced into the Log pane (§5b, §2g).
    fn drain_announce(&mut self) {
        let lines: Vec<String> = match self.announce.lock() {
            Ok(mut v) if !v.is_empty() => v.drain(..).collect(),
            _ => return,
        };
        let mut log = self.logs.viewer.borrow_mut();
        for line in lines {
            self.logs.empty = false;
            log.push(format!("fmonero: {line}"));
        }
    }
}

/// Every input in the TUI: themed, with its hint drawn at render time by
/// `screens::hinted_input` rather than handed to ftui's own empty-field text (§2h).
fn text_input(th: &Theme) -> TextInput {
    TextInput::new()
        .with_style(th.text())
        .with_cursor_style(Style::new().bg(th.accent).fg(th.selection))
}

fn restyle(old: &TextInput, th: &Theme) -> TextInput {
    let mut fresh = text_input(th).with_value(old.value().to_string());
    fresh.set_focused(old.focused());
    fresh
}

/// §2b.4: the password buffer is overwritten with zeroes when the modal closes.
fn zero(input: &mut TextInput) {
    let n = input.value().chars().count();
    input.set_value("\0".repeat(n));
    input.clear();
}

impl Model for Tui {
    type Message = Msg;

    fn init(&mut self) -> Cmd<Msg> {
        let mut cmds = vec![self.poll(), self.load_log()];
        // The picker reads the **cache** on entry (`nodes::cached`, which cannot spawn), so a fetch
        // is never on the render path and launching the TUI contacts nobody (§2n.5 rule 1, §2n.6).
        self.reload_picker();
        self.wallets = wallet_dir::list(&self.ctx).unwrap_or_default();
        match self.screen {
            Screen::Wallet => {
                cmds.push(self.load_wallet());
                cmds.push(self.load_history());
            }
            Screen::Node => cmds.push(self.load_extras()),
            _ => {}
        }
        // `wallet.autostart` (§7a.5), default **false**: the owner's problem was finding the
        // switch, not that a program had failed to guess, and a tool that starts a daemon on launch
        // has done something the user did not ask for. This is its **one** reader — `status` must
        // never start anything (§2e), and neither may the widget or the doctor.
        if self.ctx.settings.wallet.autostart && self.action.is_none() {
            cmds.push(self.start(Action::WalletDaemonStart));
        }
        Cmd::batch(cmds)
    }

    fn subscriptions(&self) -> Vec<Box<dyn Subscription<Msg>>> {
        let mut v: Vec<Box<dyn Subscription<Msg>>> = vec![
            Box::new(Every::with_id(
                1,
                Duration::from_millis(self.ctx.settings.tui.poll_ms),
                || Msg::Poll,
            )),
            Box::new(Every::with_id(
                2,
                Duration::from_millis(self.ctx.settings.tui.tick_ms),
                || Msg::Tick,
            )),
        ];
        if let Some(a) = &self.action {
            // The `Arc` is cloned, never the `Action` (§7a): `run` moves it out once.
            v.push(Box::new(ActionSub {
                id: a.id,
                ctx: self.ctx.clone(),
                slot: a.slot.clone(),
                announce: self.announce.clone(),
            }));
        }
        v
    }

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Raw(Event::Key(k)) => self.on_key(k),
            Msg::Raw(Event::Mouse(ev)) => self.on_mouse(ev),
            Msg::Raw(_) => Cmd::none(),
            Msg::Poll => {
                self.maybe_reload_theme();
                // §7a.7 part 2, before the poll that would otherwise ask `status` about the
                // node this process still thinks is configured.
                let settings_ok = self.maybe_reload_settings();
                let mut cmds = vec![self.poll()];
                if self.logs.follow && self.screen == Screen::Log {
                    cmds.push(self.load_log());
                }
                // §2n.4's cadence, and it is the **only** automatic probe in this design: the node
                // already in use, only while the Node screen is showing, only every
                // `node.remote_probe_sec` seconds, and never from `status`.
                if settings_ok {
                    cmds.push(self.maybe_probe_configured());
                }
                Cmd::batch(cmds)
            }
            Msg::Tick => {
                self.tick += 1;
                self.drain_announce();
                if !self.note.is_empty() && self.note_at.elapsed() > Duration::from_secs(10) {
                    self.note.clear();
                }
                Cmd::none()
            }
            Msg::Status(r) => {
                self.poll_inflight = false;
                match *r {
                    Ok(s) => {
                        if let Some(bps) = s.node.net.rx_bps {
                            if self.node.rx.len() == RX_SAMPLES {
                                self.node.rx.pop_front();
                            }
                            self.node.rx.push_back(bps as f64);
                        }
                        self.update_eta(&s);
                        let opened = s.wallet.open;
                        self.status_err = None;
                        self.status = Some(s);
                        // Load once when a wallet appears, and **not** again every second
                        // after a failure: a 1 Hz retry of four RPCs against a wallet that is
                        // answering errors is the pile-up §2d exists to prevent. `r` retries,
                        // and the pane says so.
                        if opened && self.wallet.addresses.is_none() && self.wallet.err.is_none() {
                            return Cmd::batch(vec![self.load_wallet(), self.load_history()]);
                        }
                        if !opened {
                            self.wallet.addresses = None;
                            self.wallet.balance = None;
                        }
                    }
                    Err(e) => self.status_err = Some(e),
                }
                Cmd::none()
            }
            Msg::Extras(r) => {
                match *r {
                    Ok(e) => {
                        self.node.err = None;
                        self.node.extras = Some(e);
                    }
                    Err(e) => self.node.err = Some(e),
                }
                Cmd::none()
            }
            Msg::WalletData(r) => {
                self.wallet.inflight = false;
                match *r {
                    Ok((b, a)) => {
                        self.wallet.err = None;
                        self.wallet.balance = Some(b);
                        self.wallet.addresses = Some(a);
                    }
                    Err(e) => self.wallet.err = Some(e),
                }
                Cmd::none()
            }
            Msg::History(r) => {
                match *r {
                    Ok((rows, wh, now)) => {
                        self.wallet.rows_err = None;
                        self.wallet.rows = rows;
                        self.wallet.wh = wh;
                        self.wallet.now = now;
                        let total = self.visible_history().len();
                        let mut st = self.wallet.list.borrow_mut();
                        st.selected = (total > 0).then(|| st.selected.unwrap_or(0).min(total - 1));
                    }
                    Err(e) => self.wallet.rows_err = Some(e),
                }
                Cmd::none()
            }
            Msg::LogLines(r) => {
                let mut reloaded = false;
                match *r {
                    Ok((lines, path)) => {
                        self.logs.path = path;
                        self.logs.empty = lines.is_empty();
                        {
                            let mut log = self.logs.viewer.borrow_mut();
                            log.clear();
                            log.push_many(lines);
                            log.set_auto_scroll(self.logs.follow);
                        }
                        reloaded = true;
                    }
                    Err(e) => self.note(e, true),
                }
                self.drain_announce();
                // `clear()` drops the viewer's match list along with its lines, and follow is on
                // by default and reloads every second — so a search typed on the Log screen was
                // wiped about a second after it matched, leaving the query on screen with no
                // highlight and `n`/`N` inert. The query lives in the model, so it is re-applied.
                if reloaded && !self.logs.search.value().trim().is_empty() {
                    return self.apply_search();
                }
                Cmd::none()
            }
            Msg::Book(entries) => {
                self.send.picker = Some((entries, 0));
                Cmd::none()
            }
            Msg::Validated(r) => match *r {
                Ok(req) => {
                    self.send.stage = Stage::Building;
                    // A refused start must return the form, not leave the screen in `Building`
                    // for the rest of the session — reachable by pressing Enter on the Send form
                    // while a `node up` is in flight.
                    match self.try_start(Action::BuildTx(req)) {
                        Ok(cmd) => cmd,
                        Err(_) => {
                            self.send.back_to_form();
                            Cmd::none()
                        }
                    }
                }
                Err(e) => {
                    self.send.back_to_form();
                    self.note(e, true);
                    Cmd::none()
                }
            },
            Msg::Task(label, r) => {
                match *r {
                    Ok(text) => {
                        // §7a: "`Ok(0)` on a clean quit; if the session's **last** action
                        // failed, that error's own `exit_code()`". Without this a transient
                        // failure early in a session — a wallet daemon still starting — left its
                        // code armed, and a clean quit after a successful retry still reported
                        // the session as failed. Every `ActionDone` success arm already does it.
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(text, false);
                        return Cmd::batch(vec![
                            self.poll(),
                            self.load_wallet(),
                            self.load_history(),
                        ]);
                    }
                    Err(e) => {
                        self.exit.store(e.code, Ordering::SeqCst);
                        // v5.6, §2o.1: the situation, then the in-app key — never the CLI's hint.
                        self.note(format!("{label}: {}", e.toast()), true);
                    }
                }
                Cmd::none()
            }
            Msg::ActionDone(r) => {
                self.action = None;
                self.quit_armed = None;
                match *r {
                    Ok(Outcome::Note(text)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(text, false);
                    }
                    Ok(Outcome::Opened(name)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(format!("{name} is open."), false);
                    }
                    Ok(Outcome::Built(plan)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.note("Checked. Nothing has been sent yet.", false);
                        self.send.typed.clear();
                        self.send.stage = Stage::Built(plan);
                        return Cmd::none();
                    }
                    Ok(Outcome::Relayed(txids)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        let first = txids.first().cloned().unwrap_or_default();
                        self.note(
                            format!("Sent. Payment {}.", fmonero_core::short_id(&first)),
                            false,
                        );
                        self.send.clear();
                        self.send.stage = Stage::Sent(first);
                        self.screen = Screen::Wallet;
                        self.tabs.borrow_mut().active = Screen::Wallet.index();
                        self.wallet.pane = Pane::History;
                        self.wallet.list.borrow_mut().selected = Some(0);
                    }
                    // ---- v5.5 -------------------------------------------------------
                    Ok(Outcome::Created { name, hint }) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(format!("{name} created and open."), false);
                        // The seed-backup modal carries `seed_backup_hint` verbatim from core —
                        // the same string the CLI prints. **fmonero never displays a seed**
                        // (§2b, §0): showing one would put it into a terminal scrollback, a tmux
                        // buffer and possibly a screen recording, for a value whose disclosure is
                        // total and irreversible loss. The modal says so, so the omission reads
                        // as a decision.
                        let mut lines: Vec<String> = hint.lines().map(str::to_string).collect();
                        lines.push(String::new());
                        lines.push(
                            "fmonero never shows your seed. A terminal's scrollback is not a \
                             safe place for one."
                                .into(),
                        );
                        if self.ctx.settings.node.mode.is_remote() {
                            // §2m.3: the NODE chose where this wallet starts scanning, and this
                            // modal is the last moment before the first refresh.
                            lines.push(
                                "This public node set where the wallet starts looking. Keep it \
                                 selected until the first check finishes."
                                    .into(),
                            );
                        }
                        self.modal = Some(Modal::SeedBackup { lines });
                        self.wallets = wallet_dir::list(&self.ctx).unwrap_or_default();
                    }
                    Ok(Outcome::NodeList(list)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.picker.progress = None;
                        self.picker.err = None;
                        self.note(
                            match list.nodes.len() {
                                1 => "1 public node in the list.".to_string(),
                                n => format!("{n} public nodes in the list."),
                            },
                            false,
                        );
                        self.picker.list = Some(*list);
                        self.sort_picker();
                    }
                    Ok(Outcome::Probes(pairs)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.picker.progress = None;
                        let reached = pairs.iter().filter(|(_, p)| p.reachable).count();
                        self.note(
                            format!("Checked {} nodes. {reached} answered.", pairs.len()),
                            false,
                        );
                        // The cache is the authority (`nodes::record_probes` wrote it), so the
                        // screen re-reads rather than patching its own copy: one place decides
                        // what a row holds.
                        self.reload_picker();
                    }
                    Ok(Outcome::NodeSwitched {
                        address,
                        must_reopen,
                    }) => {
                        self.exit.store(0, Ordering::SeqCst);
                        // §7a.7 part 1, and this is the one place it is called from: the action
                        // that wrote `config.toml` has just finished, so the model's `Context`
                        // is what is now stale.
                        if let Err(e) = self.reload_ctx() {
                            self.note(format!("Could not read your settings: {e}"), true);
                        }
                        self.node_changed_under_us();
                        match must_reopen {
                            // The reopen is offered **by name, at the moment it is needed** —
                            // the recovery v5.4's objection to a TUI mode switch asked for
                            // (§7a.3, §2m.5 step 9).
                            Some(name) => {
                                self.note(
                                    format!(
                                        "Now on {address}. {name} was closed. Press o to reopen it."
                                    ),
                                    true,
                                );
                                return self.open_wallet_modal();
                            }
                            None => self.note(format!("Now on {address}."), false),
                        }
                    }
                    Err(e) => {
                        self.exit.store(e.code, Ordering::SeqCst);
                        self.picker.progress = None;
                        if matches!(self.send.stage, Stage::Building | Stage::Relaying) {
                            self.send.back_to_form();
                        }
                        if e.interrupted {
                            self.note("Cancelled.", true);
                        } else {
                            // §2o rule 5 and §2o.1: neither the `code_name` nor the CLI's hint.
                            // The sentence, and the in-app action when the error carries one.
                            self.note(e.toast(), true);
                        }
                    }
                }
                Cmd::batch(vec![self.poll(), self.load_wallet(), self.load_history()])
            }
        }
    }

    fn view(&self, frame: &mut ftui::Frame) {
        screens::view(self, frame);
    }
}

impl Tui {
    /// Two height samples, ten seconds apart, are enough for an honest "n hours left"; with
    /// one sample there is no rate and the hero simply omits the row.
    fn update_eta(&mut self, s: &Status) {
        let (Some(h), Some(target)) = (s.node.height, s.node.target_height) else {
            return;
        };
        if s.node.synchronized == Some(true) || target <= h {
            self.node.eta = None;
            self.node.sample = None;
            return;
        }
        match self.node.sample {
            Some((at, then)) if h > then && at.elapsed() >= Duration::from_secs(10) => {
                let rate = (h - then) as f64 / at.elapsed().as_secs_f64();
                if rate > 0.0 {
                    self.node.eta = Some(((target - h) as f64 / rate) as u64);
                }
                self.node.sample = Some((Instant::now(), h));
            }
            None => self.node.sample = Some((Instant::now(), h)),
            _ => {}
        }
    }
}
