//! The FrankenTUI app: Node · Wallet · Send · Logs · Receive · Tools.
//!
//! One model polls status every second and drives cancellable actions through ActionSub.
//! Core owns wallet/node operations and all payment guarantees; the runtime owns the terminal.

mod accounts;
mod advanced;
mod annotations;
mod clipboard;
mod editor;
mod hit;
mod native;
mod privacy;
mod receiving;
mod requests;
mod screens;
mod send;
mod subs;
#[cfg(test)]
mod tests;
mod theme;
mod workflows;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use clap::Args;
use ftui::runtime::{Every, SubId, Subscription};
use ftui::widgets::input::TextInput;
use ftui::widgets::table::TableState;
use ftui::widgets::virtualized::VirtualizedListState;
use ftui::widgets::{LogViewer, LogViewerState, LogWrapMode, TabsState};
use ftui::{
    App, Cmd, Event, KeyCode, KeyEvent, KeyEventKind, Model, Modifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use fmonero_core::nodes::{self, ProbeTarget};
use fmonero_core::rpc::node::{Ban, Connection, TxpoolStats};
use fmonero_core::rpc::wallet::{AddressSet, Balance, BookEntry};
use fmonero_core::{
    history, settings as coresettings, state, status as corestatus, units, wallet_dir, Context,
    Error, HistoryQuery, NodeClient, SendRequest, Status, Transfer, TransferKind, WalletClient,
};

use editor::SecretInput;
use send::{SendCmd, SendScreen, Stage};
use subs::{Action, ActionErr, ActionSub, Outcome};
use theme::Theme;

/// TUI screen/form launch flags. Without a flag, open Wallet.
#[derive(Args, Clone, Debug, Default)]
#[group(multiple = false)]
pub struct TuiArgs {
    /// Open on the Node screen
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
    /// Open receiving addresses and payment requests
    #[arg(long)]
    pub receive: bool,
    /// Open advanced wallet tools
    #[arg(long)]
    pub tools: bool,
    /// Open the protected wallet restoration form
    #[arg(long)]
    pub restore: bool,
    /// Open the labeled receiving-address form
    #[arg(long)]
    pub new_address: bool,
    /// Open payment requests
    #[arg(long)]
    pub requests: bool,
}

/// A navigation intent never authorizes a wallet mutation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchIntent {
    #[default]
    Default,
    Node,
    Wallet,
    Send,
    Logs,
    Receive,
    Tools,
    Restore,
    NewAddress,
    Requests,
}

impl TuiArgs {
    fn intent(&self) -> LaunchIntent {
        if self.restore {
            LaunchIntent::Restore
        } else if self.new_address {
            LaunchIntent::NewAddress
        } else if self.requests {
            LaunchIntent::Requests
        } else if self.receive {
            LaunchIntent::Receive
        } else if self.tools {
            LaunchIntent::Tools
        } else if self.node {
            LaunchIntent::Node
        } else if self.send {
            LaunchIntent::Send
        } else if self.log {
            LaunchIntent::Logs
        } else {
            LaunchIntent::Wallet
        }
    }
}

impl LaunchIntent {
    fn screen(self) -> Screen {
        match self {
            Self::Node => Screen::Node,
            Self::Send => Screen::Send,
            Self::Logs => Screen::Log,
            Self::Receive | Self::NewAddress | Self::Requests => Screen::Receive,
            Self::Tools | Self::Restore => Screen::Tools,
            _ => Screen::Wallet,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    Node,
    #[default]
    Wallet,
    Send,
    Log,
    Receive,
    Tools,
}

impl Screen {
    const ALL: [Self; 6] = [
        Self::Node,
        Self::Wallet,
        Self::Send,
        Self::Log,
        Self::Receive,
        Self::Tools,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(1)
    }
    fn from_index(i: usize) -> Screen {
        Self::ALL[i % Self::ALL.len()]
    }
    fn name(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Wallet => "Wallet",
            Self::Send => "Send",
            Self::Log => "Logs",
            Self::Receive => "Receive",
            Self::Tools => "Tools",
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
    (
        "1–6 · Tab / Shift+Tab",
        "Node, Wallet, Send, Logs, Receive, Tools. Fields and dialogs keep Tab.",
    ),
    (
        "Esc",
        "Leave a field, cancel a dialog, or request cancellation of work.",
    ),
    ("q · Ctrl+C", "Quit. Work in progress asks before leaving."),
    (
        "? · v",
        "Help · shared privacy. Passwords and recovery secrets stay masked.",
    ),
    ("O · C · R", "Wallet: Open · Create · Restore."),
    (
        "N · L · Q",
        "Wallet / Receive: New address · Label selected address · QR.",
    ),
    (
        "F · X · E",
        "Wallet: Refresh payments · Close wallet · Edit payment note.",
    ),
    ("A · U", "Wallet: Accounts and contacts · Payment request."),
    (
        "Tools: ←→ · ↑↓",
        "Choose a group and action. Enter opens its protected form.",
    ),
    (
        "Send: Ctrl+B",
        "Choose a saved contact while editing the recipient.",
    ),
    (
        "↑↓ · Enter",
        "Select a row and open it. A second click also opens it.",
    ),
    (
        "Tab / Shift+Tab",
        "Forms: next / previous field. Enter submits. Esc clears secrets and cancels.",
    ),
    (
        "Send: type send",
        "Confirm only after reading every row of the exact prepared payment.",
    ),
    (
        "Review: ↑↓",
        "Scroll full payment details. Privacy reveal applies to this review only.",
    ),
    (
        "Node: u · d · p · f",
        "Start · Stop · Probe listed nodes · Fetch node list.",
    ),
    (
        "Logs: / · f · n / N",
        "Search · Follow · Next / previous match.",
    ),
    (
        "Mouse",
        "Tabs, buttons, list rows, fields and selectors use the same actions as their keys.",
    ),
];

/// Core's one sentence for "nobody has set a node up here" plus the TUI's own route (§2o.1).
///
/// One function, so the hero and the three keys that refuse in that state cannot drift — which is
/// exactly how round 5's S10 came about: four surfaces described one fact and the one a person met
/// first was the false one.
pub(super) fn node_set_up_line() -> String {
    format!("{} Press S to set it up.", fmonero_core::NODE_NOT_SET_UP)
}

/// The four fee levels, by the names the user chooses between (§2o.2). Monero's priority is
/// `0..=4` on the wire, where **0 is the daemon's own alias of 1** (§1b.4) — so there are four
/// names, and a stored `0` renders as the first of them. The Send screen's chips, the Node
/// screen's fee row and the send confirm all read from here, so the three cannot disagree.
pub(super) const FEE_LEVELS: [&str; 4] = ["cheapest", "normal", "fast", "fastest"];

/// A wire priority as its name — **from core**, so the line printed immediately before the money
/// moves uses the same four words this screen does (X2).
pub(super) use fmonero_core::send::fee_level_words as fee_level_name;

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
/// **M12: no `id`.** The line offered a filter on something the TUI never displays — ids are
/// `--json` only (§2o rule 5) — so the one search term a reader would reach for first cannot
/// match anything they can see. Both fields it names are on the rows.
pub(super) const FILTER_HINT: &str = "filter by address or note";
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
    select_address: Option<u32>,
    new_address_identity: Option<(fmonero_core::wallet_ops::WalletIdentity, u32, u64)>,
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
    pub detail_scroll: usize,
    pub detail_max: std::cell::Cell<usize>,
}

/// The public-node picker's own state (§7a.4, v5.5).
#[derive(Default)]
pub(super) struct PickerUi {
    /// The cache as `nodes::cached` read it — `None` means no cache, which renders §7a.4's
    /// no-cache panel and not an empty table with a broken footer (revision 8).
    pub list: Option<fmonero_core::NodeList>,
    /// No row is selected on entry: the first click selects a target before confirmation.
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
    pub source: fmonero_core::log_sources::LogSource,
    pub cursor: Option<fmonero_core::log_sources::LogCursor>,
    pub notice: Option<String>,
    pub viewer: RefCell<LogViewer>,
    pub state: RefCell<LogViewerState>,
    pub path: PathBuf,
    pub empty: bool,
    pub follow: bool,
    /// The TUI's own 0→4→0 cycle pointer for `L`: the daemon never reports its level.
    level: i8,
    /// What **fmonero** last set the level to in this session, or `None`. The node never reports
    /// its own, so this is the only thing the confirm may state as a fact (§2j).
    set_level: Option<i8>,
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

enum Modal {
    Native(Box<native::Review>),
    Result(advanced::ResultView),
    Accounts(accounts::Panel),
    AccountEditor(accounts::Editor),
    Workflow(workflows::Form),
    PaymentRequest(requests::Review),
    /// The renderer owns confirmation layout and hit targets.
    Confirm {
        what: Confirm,
    },
    OpenWallet {
        names: Vec<String>,
        sel: usize,
        /// Masked (§2b.4): the buffer is overwritten with zeroes when the modal closes.
        pass: SecretInput,
        asking: bool,
    },
    Prompt {
        what: Prompt,
        title: String,
        /// The field's own label (M5): an unlabelled input row was the one thing in the modal a
        /// person types into and the one thing with no name on it.
        field: String,
        input: TextInput,
    },
    Qr {
        address: String,
    },
    /// Name, password, confirmation, and language; arrows/Enter advance (§7a.5).
    CreateWallet(Box<CreateForm>),
    /// Backup guidance only; the TUI never displays a seed.
    SeedBackup {
        lines: Vec<String>,
    },
    /// Full node address and consequences before confirmation (§7a.4).
    UseNode {
        address: String,
        /// The open wallet whose connection will change, named when there is one.
        wallet_open: Option<String>,
    },
    /// Validated by Settings::parse_remote_address; Tools → Care edits transport and trust.
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

/// Boxed to keep Modal small; three text inputs and a language selector (§7a.5).
struct CreateForm {
    pub name: TextInput,
    /// Masked (§2b.4): both buffers are overwritten with zeroes when the modal closes.
    pass: SecretInput,
    again: SecretInput,
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
    /// The level `y` would set, and **the level fmonero last set, when it has set one** (round 4
    /// iteration 3). The modal named only the target, so a reader could not tell whether confirming
    /// changed anything — and naming `from` unconditionally would have been worse: the node does not
    /// report its level, so on a fresh window fmonero's own pointer is 0 and a modal reading *"Your
    /// node writes level 0 now"* would be a claim about the node it cannot make (§2j). `None` until
    /// this session has set one.
    LogLevel {
        from: Option<i8>,
        to: i8,
    },
    /// v5.6, §7a.4's `l`: confirm switching the wallet connection to your own node.
    NodeLocal,
    /// Round 5, S8: `P` on the Wallet screen, the in-app half of `wallet clear-pending`. It asks
    /// first because the record it deletes is the interlock that stops a second spend (§2a.4), and
    /// the modal states that in plain words rather than asking for a `y` against a bare fact.
    ClearPending,
}

pub(super) enum Prompt {
    NewAddress {
        wallet: fmonero_core::wallet_ops::WalletIdentity,
        account: u32,
    },
    Label {
        address: fmonero_core::receive::ReceivingAddress,
    },
    Note {
        wallet: fmonero_core::wallet_ops::WalletIdentity,
        txid: String,
        account: u32,
        expected: String,
    },
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
    cancel: fmonero_core::CancelToken,
    cancelling: bool,
}

/// The rows `history::query` returned, plus the WALLET height and local clock it derived —
/// the two `Option`s §1b.4 forbids a caller from supplying.
type HistoryRead = (Vec<Transfer>, Option<u64>, Option<i64>);

enum Msg {
    Raw(Event),
    /// 1 s: re-read `status` (dropped when one is already in flight, §2d).
    Poll,
    /// 120 ms: the spinner frame budget and the note expiry. It touches no daemon.
    Tick,
    Status(Box<Result<Status, String>>),
    StatusRead(u64, Box<Result<Status, String>>),
    Extras(Box<Result<NodeExtras, String>>),
    WalletData(u64, Box<Result<(Balance, AddressSet), String>>),
    History(u64, u64, u64, Box<Result<HistoryRead, String>>),
    Notifications(Result<(), String>),
    Received(u64, Box<Result<fmonero_core::receive::Observation, String>>),
    LogLines(Box<Result<(Vec<String>, PathBuf), String>>),
    LogSnapshot(
        fmonero_core::log_sources::LogSource,
        Box<Result<fmonero_core::log_sources::LogSnapshot, String>>,
    ),
    Book(Vec<BookEntry>),
    /// `validate_address` + the nettype check, before the Send screen builds (§7a step 2).
    Validated(Box<Result<SendRequest, String>>),
    /// A short RPC: new subaddress, label, note, log level, or a clipboard copy.
    Task(&'static str, Box<Result<String, ActionErr>>),
    ActionDone(Box<Result<Outcome, ActionErr>>),
    ActionFinished(SubId, Box<Result<Outcome, ActionErr>>),
}

impl From<Event> for Msg {
    fn from(e: Event) -> Self {
        Msg::Raw(e)
    }
}

// Include inode and size: atomic saves can share an mtime on coarse clocks.
fn file_stamp(path: &std::path::Path) -> Option<(SystemTime, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.ino(), m.len()))
}

struct Tui {
    ctx: Context,
    session: Option<fmonero_core::app::Session>,
    exit: Arc<AtomicI32>,
    theme: Theme,
    theme_stamp: Option<(SystemTime, u64, u64)>,
    /// Settings reload stamp for changes outside this window (§7a.7).
    config_mtime: Option<(SystemTime, u64, u64)>,
    config_valid: bool,
    read_epoch: u64,
    history_seq: u64,
    status_revision: u64,
    screen: Screen,
    launch_intent: LaunchIntent,
    tabs: RefCell<TabsState>,
    show_help: bool,
    help_scroll: usize,
    help_max: Cell<usize>,
    dialog_scroll: Cell<usize>,
    dialog_max: Cell<usize>,
    tools_group: usize,
    tools_selected: usize,
    tick: usize,
    status: Option<Status>,
    status_err: Option<String>,
    poll_inflight: bool,
    node: NodeUi,
    /// v5.5, §7a.4.
    picker: PickerUi,
    wallet: WalletUi,
    logs: LogUi,
    receiving: receiving::ReceiveUi,
    send: SendScreen,
    modal: Option<Modal>,
    modal_back: Option<Box<Modal>>,
    notifications_inflight: bool,
    notifications_at: Instant,
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
    render_size: std::cell::Cell<ftui::layout::Rect>,
    /// The wallets in `wallet.dir`, re-read on entry to the Wallet screen so the empty state
    /// can say `0 wallets` — a real count, so a user can see why `o` will not help before
    /// pressing it (§7a.5).
    wallets: Vec<String>,
    /// **Why that list is empty, when it is empty because the folder could not be read**
    /// (round 4, S1).
    ///
    /// All four `wallet_dir::list` call sites ended `.unwrap_or_default()`, so a folder fmonero
    /// was not allowed to read became a folder with no wallets in it, and the Wallet screen then
    /// told the owner *"none on this machine yet. Press c to create one"* — a false statement
    /// about his money followed by the one action that writes a new wallet file where the old one
    /// lives. The CLI on the identical state was honest. This holds core's own sentence for the
    /// two places that would otherwise substitute one.
    wallets_err: Option<String>,
}

/// Run the TUI. `Ok(0)` on a clean quit, the §4a code of the session's last failed action
/// otherwise, `Ok(130)` on Ctrl+C — the TUI never invents an exit code (§7a).
pub fn run(args: TuiArgs, ctx: Context) -> anyhow::Result<i32> {
    let intent = args.intent();
    run_with_intent(args, ctx, intent)
}

pub fn run_with_intent(_args: TuiArgs, ctx: Context, intent: LaunchIntent) -> anyhow::Result<i32> {
    let exit = Arc::new(AtomicI32::new(0));
    // Mouse capture is configurable; terminal selection modifiers remain terminal-specific.
    let mouse = ctx.settings.tui.mouse;
    let session = fmonero_core::app::Session::enter(&ctx)?;
    let mut app = Tui::new(intent.screen(), ctx, exit.clone());
    app.session = Some(session);
    app.launch_intent = intent;
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
            session: None,
            theme_stamp: file_stamp(&theme::colors_path()),
            config_mtime: file_stamp(&ctx.paths.config_file()),
            config_valid: true,
            read_epoch: 0,
            history_seq: 0,
            status_revision: 0,
            node: NodeUi::default(),
            picker: PickerUi {
                filter: text_input(&theme),
                ..PickerUi::default()
            },
            wallet: WalletUi {
                filter: text_input(&theme),
                ..WalletUi::default()
            },
            receiving: receiving::ReceiveUi::default(),
            logs: LogUi {
                source: fmonero_core::log_sources::LogSource::Wallet,
                cursor: None,
                notice: None,
                viewer: RefCell::new(LogViewer::new(LOG_LINES).wrap_mode(LogWrapMode::NoWrap)),
                state: RefCell::new(LogViewerState::default()),
                path: ctx.settings.node.log_file.clone(),
                empty: true,
                follow: true,
                level: 0,
                set_level: None,
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
            launch_intent: LaunchIntent::Default,
            show_help: false,
            help_scroll: 0,
            help_max: Cell::new(0),
            dialog_scroll: Cell::new(0),
            dialog_max: Cell::new(0),
            tools_group: 0,
            tools_selected: 0,
            tick: 0,
            status: None,
            status_err: None,
            poll_inflight: false,
            modal: None,
            modal_back: None,
            notifications_inflight: false,
            notifications_at: Instant::now(),
            announce: Arc::new(Mutex::new(Vec::new())),
            action: None,
            sub_seq: 0,
            quit_armed: None,
            note: String::new(),
            note_err: false,
            note_at: Instant::now(),
            hits: hit::Map::default(),
            render_size: std::cell::Cell::new(ftui::layout::Rect::new(0, 0, 80, 24)),
            wallets: Vec::new(),
            wallets_err: None,
        }
    }

    // ------------------------------------------------------------ what the screens ask

    pub(super) fn account_index(&self) -> u32 {
        self.ctx.settings.wallet.account
    }

    fn invalidate_wallet_reads(&mut self) {
        self.read_epoch += 1;
        self.receiving = receiving::ReceiveUi::default();
        self.poll_inflight = false;
        self.wallet.inflight = false;
        self.wallet.balance = None;
        self.wallet.addresses = None;
        self.wallet.rows.clear();
        self.wallet.wh = None;
        self.wallet.now = None;
        self.wallet.err = None;
        self.wallet.rows_err = None;
        self.wallet.detail = false;
        self.wallet.table.borrow_mut().selected = None;
        self.wallet.list.borrow_mut().selected = None;
    }

    /// The node badge as **the word a person reads** and the **tone the theme paints it**
    /// (§2o rule 8, v5.6). The two were one string until v5.6, which is why the badge said
    /// `down` for a node that may be perfectly well and merely unreachable: the word had to
    /// double as `Theme::badge`'s style key. The tone keys are internal and never rendered.
    fn node_state(&self) -> (&'static str, &'static str) {
        let Some(s) = &self.status else {
            return ("not answering", "down");
        };
        // **A pending payment is not a fault of the node** (round 4 iteration 3). `s.pending`
        // was in this condition, so a node that was synced, answering and holding nineteen peers
        // reported `● node problem` in the three-cell summary a person reads on every screen —
        // two inches under a hero reading `● Synced · blocks ████ 100% · peers 16 out · 3 in`.
        // The reader's own conclusion: *"he will go and look at his node while the thing that
        // needs attention is a payment."* A payment belongs to the wallet cell
        // ([`Tui::wallet_state`]), and the red banner above every screen is where it is loudest.
        if self.status_err.is_some() || self.has_warning("node_stalled") {
            return ("problem", "error");
        }
        // Remote mode answers first (§2m.3): `reachable` and `synchronized` are both `null`
        // there, so every badge below would paint "not answering" for a wallet that is working.
        //
        // **X13: the cached check's verdict belongs here too.** With a public node that had been
        // checked and had not answered, the badge read `● public node …` with no verdict at all and
        // the hero the same: the one fact the reader needed was in the ` This node ` pane and
        // nowhere else, because W9's one predicate covers your own node only. The words are the
        // same three the pane and `status` use, from §5's `node.remote_probe`.
        if self.remote_mode() {
            return match self.remote_check_words() {
                Some(words @ "no answer") => (words, "down"),
                Some(words @ "syncing") => (words, "syncing"),
                Some(words) => (words, "up"),
                None => ("public node", "up"),
            };
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

    /// **X13: the verdict of the last check of the configured public node, in three words.**
    ///
    /// `None` when there is no check yet, so a caller can say "not checked yet" in its own shape.
    /// The badge, the hero and the ` This node ` pane all read this, which is what makes the frame
    /// say one thing: before it, only the pane had a verdict.
    pub(super) fn remote_check_words(&self) -> Option<&'static str> {
        let p = self.status.as_ref()?.node.remote_probe.as_ref()?;
        if !p.reachable {
            // **M3: the specific word, not `no answer` for all six.** This collapsed every failure
            // while the picker's own column two panes away rendered `refused` from the same field
            // — so the more useful word was the one hidden from the hero, the badge and the pane.
            // `probe_reason_words` is the one table (§2n.4) and it already answers `no answer` for
            // a reason it does not recognise.
            return Some(fmonero_core::nodes::probe_reason_words(p.reason.as_deref()));
        }
        Some(match p.synchronized {
            Some(true) => "synced",
            Some(false) => "syncing",
            None => "answered",
        })
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
        // **The pending payment lands here, not on the node cell** (round 4 iteration 3): it is
        // money, and the wallet is the cell a person reads for money. The words are the badge's
        // own register — three of them, and they name the fact rather than a count.
        if s.pending.is_some() {
            return ("payment may be out", "error");
        }
        if self.has_warning("wallet_stalled") {
            return ("problem", "error");
        }
        if s.wallet.busy {
            return ("busy", "syncing");
        }
        let ours = self.action.as_ref().is_some_and(|a| a.wallet_busy);
        match (s.wallet.reachable, s.wallet.open) {
            // **`starting`/`busy`, not `down`, while one of our own actions is the reason a poll
            // cannot answer** (DECISIONS: FINDING D). `W` and a TUI create both ran under a badge
            // reading `down` two lines below a panel reading `starting…`: an assertion about a
            // daemon this model knew to be false. `down` survives only for the state it describes
            // — nothing of ours in flight and the poll still failing, which core has already split
            // into `wallet_idle` (deliberately not running, above) and `wallet_down`.
            // **S9: one word for "fmonero is doing something", so the badge does not invent a
            // second description.** `scanning` was printed while a wallet was being CREATED, in a
            // frame whose status bar and whose Wallet pane both named the create. The badge is the
            // three-cell summary of the frame; the label beside the spinner is where the detail is.
            (false, _) if ours => ("working", "syncing"),
            // **V13: the service is running and it is not answering, and the screen said both.**
            // The Wallet screen read `wallet service: running` (which is `unit_active`) four rows
            // above a status bar reading `wallet not running` (which was `reachable`), and both
            // were computed correctly. `not running` is now kept for the state it describes, and a
            // service that is up without answering says so — the same split §2c makes between
            // `unreachable` and a service that is simply not there.
            (false, _) if s.wallet.unit_active => ("not answering", "down"),
            // **One state, one word** (round 4 iteration 3). This said `closed` where the
            // `wallet_idle` warning was raised and `not running` where it was not, so one fact —
            // the service is down and nothing is open — read two ways depending on which mode
            // the reader was in, and the Wallet screen's own row in the same frame said a third
            // thing: `not running yet. It starts when you open or create a wallet.` That row is
            // the wording. Calm-versus-alarm is `wallet_idle`'s job and the bar icon's, not this
            // word's: both branches were the same `down` tone anyway.
            (false, _) => ("not running", "down"),
            (true, true) if screens::wallet_sync_line(s).is_some() => ("syncing", "syncing"),
            (true, true) => ("open", "up"),
            (true, false) => ("closed", "down"),
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
            // X13: the verdict, from the same predicate the badge reads. `Public node ·
            // public.example.net:18089` with a check that had not answered was the hero saying
            // nothing about the one thing that was wrong.
            return match self.remote_check_words() {
                Some(words) => format!("Public node · {words} · {}", s.node.url),
                None => format!("Public node · not checked yet · {}", s.node.url),
            };
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
            // X11: one rounding, from core.
            (_, Some(p)) => format!("Syncing {}", fmonero_core::percent(p)),
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
        if self.poll_inflight || !self.config_valid {
            return Cmd::none();
        }
        self.poll_inflight = true;
        let epoch = self.read_epoch;
        let ctx = self.ctx.clone();
        Cmd::task(move || {
            Msg::StatusRead(
                epoch,
                Box::new(corestatus::status_checking_node(&ctx).map_err(|e| e.to_string())),
            )
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
        if self.wallet.inflight || !self.config_valid {
            return Cmd::none();
        }
        self.wallet.inflight = true;
        let epoch = self.read_epoch;
        let ctx = self.ctx.clone();
        let account = self.account_index();
        Cmd::task(move || {
            let read = || -> Result<(Balance, AddressSet), Error> {
                let w = WalletClient::from_ctx(&ctx)?;
                Ok((w.get_balance(account)?, w.get_address(account)?))
            };
            Msg::WalletData(epoch, Box::new(read().map_err(|e| e.to_string())))
        })
    }

    fn load_history(&mut self) -> Cmd<Msg> {
        if !self.config_valid {
            return Cmd::none();
        }
        self.history_seq += 1;
        let seq = self.history_seq;
        let epoch = self.read_epoch;
        let revision = self.status_revision;
        let ctx = self.ctx.clone();
        let account = self.account_index();
        // X17: the same default set `wallet history` uses, from one constant in core.
        //
        // **The kind filter no longer narrows the QUERY** (round 4 iteration 3). It did, so with
        // `t` on "received" the pane held only the received rows and its own title read
        // `Payments  2 of 2 · in only` on a wallet holding six — "2 of 2" reads as *you have two*.
        // It also made S5's third empty state unreachable: with a kind that matches nothing the
        // query came back empty, so the pane fell to "Nothing yet. Press R to check" instead of
        // "You have 6 payments, and none of them is sent." One query, the default set, and
        // `visible_history` does the filtering — which it already did, which is why the count was
        // filtered twice. `Pool` is added when it is the kind asked for, because it is the one
        // kind the default set leaves out (it needs the node).
        let mut kinds = TransferKind::DEFAULT_ROWS.to_vec();
        if let Some(k) = self.wallet.kind {
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
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
            Msg::History(
                epoch,
                seq,
                revision,
                Box::new(read().map_err(|e| e.to_string())),
            )
        })
    }

    fn load_log(&self) -> Cmd<Msg> {
        let ctx = self.ctx.clone();
        let source = self.logs.source;
        let query = fmonero_core::log_sources::LogQuery {
            source,
            lines: LOG_LINES,
            search: None,
            previous: self.logs.cursor.clone(),
        };
        Cmd::task(move || {
            Msg::LogSnapshot(
                source,
                Box::new(
                    fmonero_core::log_sources::read(
                        &ctx,
                        &query,
                        &fmonero_core::CancelToken::new(),
                    )
                    .map_err(|e| e.to_string()),
                ),
            )
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
    fn start_for_test(&mut self, action: Action) {
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
        let held_native = matches!(&action, Action::Native(job) if job.held());
        if !self.config_valid && !matches!(action, Action::Relay(_)) && !held_native {
            self.note("Fix your settings before starting another action.", true);
            return Err(Box::new(action));
        }
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
            Action::Native(ref job) if job.held() => None,
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
            cancel: fmonero_core::CancelToken::new(),
            cancelling: false,
            started: Instant::now(),
            slot: Arc::new(Mutex::new(Some(action))),
        });
        Ok(Cmd::none())
    }

    /// `Esc`, or `q` twice: drop the subscription, which raises the `StopSignal`, which trips
    /// the `CancelToken`. A relay is never cancelled (§7a).
    fn cancel_action(&mut self) -> Cmd<Msg> {
        let (message, error) = match self.action.as_mut() {
            None => ("Nothing to cancel.".to_string(), false),
            Some(a) if !a.cancellable => (
                "Submission is awaiting its result. It cannot safely be cancelled or retried."
                    .into(),
                true,
            ),
            Some(a) => {
                a.cancel.cancel();
                a.cancelling = true;
                ("Cancellation requested. Waiting for the backend outcome before another action.".into(), false)
            }
        };
        self.note(message, error);
        Cmd::none()
    }

    /// **Re-read `wallet.dir`, keeping the reason when it cannot be read** (round 4, S1).
    ///
    /// The one place the TUI learns what wallets exist. Four call sites each did this with
    /// `.unwrap_or_default()`, which turned "fmonero is not allowed to read your wallet folder"
    /// into "you have no wallets" — and then the screen offered to create one, in the folder
    /// holding the one it could not see. An error keeps the list it last had rather than emptying
    /// it, for the same reason.
    fn reload_wallets(&mut self) {
        match wallet_dir::list(&self.ctx) {
            Ok(names) => {
                self.wallets = names;
                self.wallets_err = None;
            }
            Err(e) => self.wallets_err = Some(e.to_string()),
        }
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
    pub(super) fn picker_rows(&self) -> Vec<&fmonero_core::PublicNode> {
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

    /// `Enter` on the picker confirms the selected target. Selecting the configured target
    /// again is allowed: it retries a connection whose previous application failed or timed out.
    fn use_selected_node(&mut self) -> Cmd<Msg> {
        let Some(sel) = self.picker.table.borrow().selected else {
            self.note("Select a node first, with ↑↓ or a click, then Enter.", true);
            return Cmd::none();
        };
        let Some(address) = self.picker_rows().get(sel).map(|n| n.address.clone()) else {
            return Cmd::none();
        };
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
    /// Confirm the connection change, including retries of the current local target.
    fn use_local(&mut self) -> Cmd<Msg> {
        self.confirm_local()
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
    /// in a toast. Public-sensitive values stay masked and confirmation is revoked until
    /// the next valid settings read.
    fn reload_ctx(&mut self) -> Result<(), Error> {
        let settings = match coresettings::Settings::load(&self.ctx.paths) {
            Ok(settings) => settings,
            Err(e) => {
                self.config_valid = false;
                self.revoke_review_authorization();
                self.invalidate_wallet_reads();
                return Err(e);
            }
        };
        let timeouts = coresettings::Timeouts::from_settings(&settings.timeouts);
        if self.ctx.settings != settings {
            self.revoke_review_authorization();
            self.invalidate_wallet_reads();
        }
        self.config_valid = true;
        self.ctx.settings = settings;
        self.ctx.timeouts = timeouts;
        self.config_mtime = file_stamp(&self.ctx.paths.config_file());
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
        let m = file_stamp(&self.ctx.paths.config_file());
        if m == self.config_mtime {
            return self.config_valid;
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
        let stamp = file_stamp(&theme::colors_path());
        if stamp != self.theme_stamp {
            self.theme_stamp = stamp;
            self.theme = Theme::load();
            self.picker.filter = restyle(&self.picker.filter, &self.theme);
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
            // **Round 5, S12: the send confirm says it in the modal, because no toast can.**
            // `screens::toast_hidden` suppresses every toast while a transaction is built, which is
            // right — the window on top states the situation — and it meant this sentence reached
            // nobody on the one screen that needed it. The create-wallet modal already carries its
            // own version of the exception on the line under its buttons (§7a.1).
            self.send.tab_note = self.send.confirming();
            self.note("Press Esc first to discard this.", true);
            return Cmd::none();
        }
        self.show_help = false;
        if self.modal.is_some() {
            return self.on_modal_key(KeyEvent::new(if forward {
                KeyCode::Tab
            } else {
                KeyCode::BackTab
            }));
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
        let delta = if forward { 1 } else { Screen::ALL.len() - 1 };
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
                let probe = self.poll();
                if self.node.extras.is_none() {
                    Cmd::batch(vec![self.load_extras(), probe])
                } else {
                    probe
                }
            }
            Screen::Wallet | Screen::Receive => {
                if s == Screen::Receive {
                    self.wallet.pane = Pane::Subaddresses;
                }
                // A real count, so the empty state can say `0 wallets` and a user can see why `o`
                // will not help before pressing it (§7a.5) — and S1's reason when there is none.
                self.reload_wallets();
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
        if self.privacy() {
            self.note(
                "Privacy hides this value. Turn privacy off explicitly before copying.",
                true,
            );
            return;
        }
        match clipboard::copy(text) {
            Ok(()) => self.note("Copied.", false),
            Err(why) => self.note(why, true),
        }
    }

    fn quit(&mut self, code: i32) -> Cmd<Msg> {
        if matches!(self.modal, Some(Modal::Native(_)))
            && !self
                .quit_armed
                .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(2))
        {
            self.quit_armed = Some((Instant::now(), code));
            self.note("Native review is held. Esc declines and reopens the wallet; quit again leaves it closed for explicit recovery.", true);
            return Cmd::none();
        }

        if let Some(a) = &self.action {
            let armed = self
                .quit_armed
                .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(2));
            if !armed {
                self.quit_armed = Some((Instant::now(), code));
                let extra = if a.cancellable {
                    "Esc cancels it, or press quit again to cancel and quit"
                } else {
                    "Submission is in progress. Leaving cannot cancel it; its result may be uncertain."
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

    // A paste is text, never a sequence of shortcuts or an implicit Enter.
    fn on_paste(&mut self, event: &Event) -> Cmd<Msg> {
        if screens::tier_of(self.render_size.get()) == screens::Tier::Tiny {
            return Cmd::none();
        }
        if let Some(modal) = self.modal.as_mut() {
            match modal {
                Modal::Workflow(form) => form.event(event),
                Modal::AccountEditor(editor) => editor.event(event),
                Modal::Prompt { input, .. } | Modal::AddNode { input } => {
                    input.handle_event(event);
                }
                Modal::OpenWallet {
                    pass, asking: true, ..
                } => {
                    pass.handle_event(event);
                }
                Modal::CreateWallet(form) => match form.field {
                    0 => {
                        form.name.handle_event(event);
                    }
                    1 => {
                        form.pass.handle_event(event);
                    }
                    2 => {
                        form.again.handle_event(event);
                    }
                    _ => {}
                },
                _ => {}
            }
        } else if !self.show_help && !self.send.confirming() {
            match self.screen {
                Screen::Send => self.send.on_paste(event),
                Screen::Wallet if self.wallet.filter_open && !self.wallet.detail => {
                    self.wallet.filter.handle_event(event);
                    self.reveal();
                }
                Screen::Node if self.picker.filter_open => {
                    self.picker.filter.handle_event(event);
                    self.picker.table.borrow_mut().select(None);
                }
                Screen::Log if self.logs.search_open && self.logs.search.handle_event(event) => {
                    return self.apply_search();
                }
                _ => {}
            }
        }
        Cmd::none()
    }

    fn on_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        if k.kind == KeyEventKind::Release {
            return Cmd::none();
        }
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(Modifiers::CTRL);
        if ctrl_c {
            return self.quit(130);
        }
        if screens::tier_of(self.render_size.get()) == screens::Tier::Tiny
            && k.code != KeyCode::Escape
        {
            return Cmd::none();
        }
        if self.modal.is_some() {
            return self.on_modal_key(k);
        }
        self.dialog_scroll.set(0);
        if self.show_help {
            match k.code {
                KeyCode::Down | KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(1).min(self.help_max.get());
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::End => self.help_scroll = self.help_max.get(),
                _ => self.show_help = false,
            }
            return Cmd::none();
        }
        // Confirmations retain keyboard ownership even after a screen change.
        if self.send.confirming() || (self.screen == Screen::Send && self.send.owns_keys()) {
            return self.on_send_key(k);
        }
        if self.wallet.detail {
            match k.code {
                KeyCode::Escape | KeyCode::Enter => {
                    self.wallet.detail = false;
                    return Cmd::none();
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    return self.click_button(hit::Button::PaymentNote)
                }
                KeyCode::Down | KeyCode::PageDown => {
                    self.wallet.detail_scroll = self
                        .wallet
                        .detail_scroll
                        .saturating_add(1)
                        .min(self.wallet.detail_max.get())
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.wallet.detail_scroll = self.wallet.detail_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return Cmd::none();
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

        if self.screen == Screen::Node && self.picker.filter_open {
            return self.on_node_key(k);
        }
        if matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            return self.switch_screen(k.code == KeyCode::Tab);
        }
        match k.code {
            KeyCode::Char('q') => return self.quit(self.exit.load(Ordering::SeqCst)),
            KeyCode::Char('v') | KeyCode::Char('V') => return self.toggle_privacy(),
            KeyCode::Char('?') => {
                self.help_scroll = 0;
                self.show_help = true;
                return Cmd::none();
            }
            KeyCode::Escape => return self.cancel_action(),
            KeyCode::Char('1') => return self.set_screen(Screen::Node),
            KeyCode::Char('2') => return self.set_screen(Screen::Wallet),
            KeyCode::Char('3') => return self.set_screen(Screen::Send),
            KeyCode::Char('4') => return self.set_screen(Screen::Log),
            KeyCode::Char('5') => return self.set_screen(Screen::Receive),
            KeyCode::Char('6') => return self.set_screen(Screen::Tools),
            KeyCode::Char('r')
                if !matches!(
                    self.screen,
                    Screen::Tools | Screen::Wallet | Screen::Receive
                ) =>
            {
                let mut cmds = vec![self.poll()];
                match self.screen {
                    Screen::Node => cmds.push(self.load_extras()),
                    Screen::Wallet | Screen::Receive => {
                        self.wallet.err = None;
                        self.wallet.rows_err = None;
                        cmds.push(self.load_wallet());
                        cmds.push(self.load_history());
                    }
                    Screen::Log => cmds.push(self.load_log()),
                    Screen::Send | Screen::Tools => {}
                }
                return Cmd::batch(cmds);
            }
            _ => {}
        }
        match self.screen {
            Screen::Node => self.on_node_key(k),
            Screen::Wallet | Screen::Receive => self.on_wallet_key(k),
            Screen::Tools => self.on_tools_key(k),
            Screen::Log => self.on_log_key(k),
            Screen::Send => {
                // The form is unfocused: Enter returns to it. `Tab` never reaches here (§7a.1).
                match k.code {
                    KeyCode::Enter => self.send.resume(),
                    KeyCode::Char('U') => return self.click_button(hit::Button::ParseRequest),
                    KeyCode::Char('B') => return self.click_button(hit::Button::SendBook),
                    KeyCode::Char('A') => return self.click_button(hit::Button::SendAll),
                    _ => {}
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
            hit::Target::Arrow { which, delta } => self.click_arrow(which, delta),
            hit::Target::Button(b) => self.click_button(b),
            hit::Target::ModalButton(index) => {
                if index == 1 {
                    return if self.send.confirming() {
                        self.on_send_key(KeyEvent::new(KeyCode::Escape))
                    } else {
                        self.on_modal_key(KeyEvent::new(KeyCode::Escape))
                    };
                }
                if self.send.confirming() {
                    return self.on_send_key(KeyEvent::new(KeyCode::Enter));
                }
                if let Some(Modal::Workflow(form)) = self.modal.as_mut() {
                    form.focus(form.fields.len() - 1);
                }
                if let Some(Modal::CreateWallet(form)) = self.modal.as_mut() {
                    form.field = 3;
                }
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
            self.note("Press Esc first to discard this.", true);
            return Cmd::none();
        }
        if self.modal.is_some() {
            return self.on_modal_key(KeyEvent::new(KeyCode::Escape));
        }
        if self.wallet.detail {
            return self.on_key(KeyEvent::new(KeyCode::Escape));
        }
        if self.wallet.filter_open || self.logs.search_open || self.send.editing {
            // **M22: Esc on the log search clears it.** It left the text in the field, so the next
            // `/` appended to it — a second search for `zzz` read `/ Syncedzzz` and matched nothing
            // for a reason nothing on screen explained. Esc leaves a search the same way it leaves
            // everything else: with nothing kept.
            let was_log = self.logs.search_open;
            self.wallet.filter_open = false;
            self.logs.search_open = false;
            self.wallet.filter.set_focused(false);
            self.logs.search.set_focused(false);
            self.send.editing = false;
            if was_log {
                self.logs.search = text_input(&self.theme);
                return self.apply_search();
            }
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
            hit::HitPane::ModalList => {
                let code = if up { KeyCode::Up } else { KeyCode::Down };
                for _ in 0..rows {
                    if self.modal.is_some() {
                        let _ = self.on_modal_key(KeyEvent::new(code));
                    } else if self.send.picker.is_some() {
                        let _ = self.on_send_key(KeyEvent::new(code));
                    }
                }
            }
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
                    self.wallet.detail_scroll = 0;
                }
                Cmd::none()
            }
            hit::HitPane::NodeList => {
                self.node.table.borrow_mut().select(Some(row));
                Cmd::none()
            }
            hit::HitPane::ModalList => {
                if let Some(Modal::Accounts(panel)) = &mut self.modal {
                    if row < panel.count() {
                        let same = panel.selected == row;
                        panel.selected = row;
                        if same {
                            return self.on_modal_key(KeyEvent::new(KeyCode::Enter));
                        }
                    }
                    return Cmd::none();
                }
                if let Some(Modal::OpenWallet {
                    names,
                    sel,
                    asking,
                    pass,
                }) = &mut self.modal
                {
                    if row < names.len() {
                        let same = *sel == row;
                        *sel = row;
                        if same {
                            *asking = true;
                            pass.set_focused(true);
                        }
                    }
                } else if let Some((entries, sel)) = &mut self.send.picker {
                    if row < entries.len() {
                        *sel = row;
                        return self.on_send_key(KeyEvent::new(KeyCode::Enter));
                    }
                }
                Cmd::none()
            }
            hit::HitPane::Log => Cmd::none(),
        }
    }

    /// A click on an input: focus it. On the Send screen that is a form field; inside a modal it
    /// is that modal's nth control.
    fn click_field(&mut self, index: usize) -> Cmd<Msg> {
        match &mut self.modal {
            Some(Modal::Workflow(form)) => form.focus(index),
            Some(Modal::AccountEditor(editor)) => editor.focus(index),
            Some(Modal::OpenWallet { pass, asking, .. }) => {
                *asking = true;
                pass.set_focused(true);
            }
            Some(Modal::Prompt { input, .. }) => input.set_focused(true),
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
            hit::Selector::Priority => {
                self.send.priority = if delta < 0 {
                    prev_fee_level(self.send.priority)
                } else {
                    next_fee_level(self.send.priority)
                };
            }
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
        if self.modal.is_none() {
            self.dialog_scroll.set(0);
        }
        match b {
            hit::Button::DialogUp => self.on_modal_key(KeyEvent::new(KeyCode::Up)),
            hit::Button::DialogDown => self.on_modal_key(KeyEvent::new(KeyCode::Down)),
            hit::Button::Search => {
                match self.screen {
                    Screen::Node => {
                        self.picker.filter_open = true;
                        self.picker.filter.set_focused(true);
                    }
                    Screen::Wallet | Screen::Receive => {
                        self.wallet.pane = Pane::History;
                        self.wallet.filter_open = true;
                        self.wallet.filter.set_focused(true);
                    }
                    Screen::Log => {
                        self.logs.search_open = true;
                        self.logs.search.set_focused(true);
                    }
                    _ => {}
                }
                Cmd::none()
            }
            hit::Button::SetupUnits => self.setup_units(),
            hit::Button::DetailUp => self.on_key(KeyEvent::new(KeyCode::Up)),
            hit::Button::DetailDown => self.on_key(KeyEvent::new(KeyCode::Down)),
            hit::Button::DetailClose => self.on_key(KeyEvent::new(KeyCode::Escape)),
            hit::Button::NativeReveal => self.on_modal_key(KeyEvent::new(KeyCode::Char('V'))),
            hit::Button::NativeFocus | hit::Button::ReviewFocus => Cmd::none(),
            hit::Button::TrackReceipts => self.track_receipts(),
            hit::Button::ReceiptDetails => self.receipt_details(),
            hit::Button::LogInfo => self.on_log_key(KeyEvent::new(KeyCode::Char('I'))),
            hit::Button::LogSource => self.on_log_key(KeyEvent::new(KeyCode::Char('s'))),
            hit::Button::LogFollow => self.on_log_key(KeyEvent::new(KeyCode::Char('f'))),
            hit::Button::LogSearch => self.on_log_key(KeyEvent::new(KeyCode::Char('/'))),
            hit::Button::LogNext => self.on_log_key(KeyEvent::new(KeyCode::Char('n'))),
            hit::Button::LogPrevious => self.on_log_key(KeyEvent::new(KeyCode::Char('N'))),
            hit::Button::ToolsUp => self.on_tools_key(KeyEvent::new(KeyCode::Up)),
            hit::Button::ToolsDown => self.on_tools_key(KeyEvent::new(KeyCode::Down)),
            hit::Button::Tool(tool) => self.open_tool(tool),
            hit::Button::ToolGroup(group) => {
                self.tools_group = group;
                self.tools_selected = 0;
                Cmd::none()
            }
            hit::Button::ResultUp => self.on_modal_key(KeyEvent::new(KeyCode::Up)),
            hit::Button::ResultDown => self.on_modal_key(KeyEvent::new(KeyCode::Down)),
            hit::Button::ResultCopy => self.on_modal_key(KeyEvent::new(KeyCode::Char('C'))),
            hit::Button::Notifications => self.toggle_notifications(),
            hit::Button::Accounts => self.start(Action::Accounts(accounts::Job::Read)),
            hit::Button::AccountSwitch => self.on_modal_key(KeyEvent::new(KeyCode::Char('B'))),
            hit::Button::AccountInfo => self.on_modal_key(KeyEvent::new(KeyCode::Char('I'))),
            hit::Button::AccountNew => self.on_modal_key(KeyEvent::new(KeyCode::Char('N'))),
            hit::Button::AccountEdit => self.on_modal_key(KeyEvent::new(KeyCode::Char('E'))),
            hit::Button::AccountDelete => self.on_modal_key(KeyEvent::new(KeyCode::Char('D'))),
            hit::Button::CreateRequest => self.open_request(false),
            hit::Button::ParseRequest => self.open_request(true),
            hit::Button::RequestUp => self.on_modal_key(KeyEvent::new(KeyCode::Up)),
            hit::Button::RequestDown => self.on_modal_key(KeyEvent::new(KeyCode::Down)),
            hit::Button::RequestQr => self.on_modal_key(KeyEvent::new(KeyCode::Char('Q'))),
            hit::Button::NewAddress => self.new_address_modal(),
            hit::Button::LabelAddress => self.prompt_label(),
            hit::Button::PaymentNote => {
                self.wallet.detail = false;
                self.prompt_note()
            }
            hit::Button::CopyAddress => {
                if let Some(address) = self.selected_address() {
                    self.copy(&address);
                }
                Cmd::none()
            }
            hit::Button::Privacy => self.toggle_privacy(),
            hit::Button::SendAll => {
                self.send.amount.set_value("all");
                self.send.resume();
                Cmd::none()
            }
            hit::Button::SendBook => self.load_book(),
            hit::Button::PreparePayment => self.on_send_key(KeyEvent::new(KeyCode::Enter)),
            hit::Button::RevealPayment => {
                if self.policy_current() && matches!(self.send.stage, Stage::Built(_)) {
                    self.send.review_revealed = true;
                }
                Cmd::none()
            }
            hit::Button::ReviewUp => self.on_send_key(KeyEvent::new(KeyCode::PageUp)),
            hit::Button::ReviewDown => self.on_send_key(KeyEvent::new(KeyCode::PageDown)),
            hit::Button::Restore => self.open_restore(),
            hit::Button::HelpUp => self.on_key(KeyEvent::new(KeyCode::Up)),
            hit::Button::HelpDown => self.on_key(KeyEvent::new(KeyCode::Down)),
            hit::Button::Help => {
                self.help_scroll = 0;
                self.show_help = !self.show_help;
                Cmd::none()
            }
            hit::Button::NextScreen => self.switch_screen(true),
            hit::Button::OpenWallet => self.open_wallet_modal(),
            hit::Button::CreateWallet => self.create_wallet_modal(),
            hit::Button::CloseWallet => self.close_wallet(),
            hit::Button::WalletDaemon => self.toggle_wallet_daemon(),
            hit::Button::RefreshWallet => self.start(Action::WalletRefresh { start_height: 0 }),
            hit::Button::ClearPending => self.confirm_clear_pending(),
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
            // M16: the same code path the `b` key takes.
            hit::Button::SwapPeers => self.swap_peers_view(),
            // M16's shape on the Payments pane: the same code path `t` takes.
            hit::Button::KindFilter => self.cycle_kind(),
        }
    }

    /// `t` — step through which kinds of payment show (M16's chip).
    fn cycle_kind(&mut self) -> Cmd<Msg> {
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
        self.load_history()
    }

    /// `b` — swap the peers table for the blocked addresses, and back (M16's chip).
    fn swap_peers_view(&mut self) -> Cmd<Msg> {
        self.node.view = match self.node.view {
            NodeView::Peers => NodeView::Bans,
            NodeView::Bans => NodeView::Peers,
        };
        self.load_extras()
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
                return self.confirm(Confirm::LogLevel {
                    from: self.logs.set_level,
                    to: next,
                });
            }
            // Round 5, S10: the in-app route that writes the node and wallet services. It is the
            // same core call `fmonero setup --units --yes` makes, and it is the only way out of
            // `Layout::None` from inside the app.
            KeyCode::Char('S') => return self.setup_units(),
            KeyCode::Char('b') => return self.swap_peers_view(),
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
            KeyCode::Char('T') | KeyCode::Char('t') if self.screen == Screen::Receive => {
                return self.track_receipts()
            }
            KeyCode::Char('I') | KeyCode::Char('i') if self.screen == Screen::Receive => {
                return self.receipt_details()
            }
            // `←→` swaps the Wallet's panes. `Tab` never reaches here: §7a.1 resolves it in
            // `on_key`'s first arm, on every screen and in every focus state.
            KeyCode::Left | KeyCode::Right if self.screen == Screen::Wallet => {
                self.wallet.pane = match self.wallet.pane {
                    Pane::Subaddresses => Pane::History,
                    Pane::History => Pane::Subaddresses,
                };
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                return self.click_button(hit::Button::OpenWallet)
            }
            // **`c` is create and `x` closes** (§7a.5). The old binding was the collision that
            // made the one key that looks like "create" do nothing visible on a machine with no
            // wallets — half of the owner's complaint (4).
            KeyCode::Char('c') | KeyCode::Char('C') => {
                return self.click_button(hit::Button::CreateWallet)
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                return self.click_button(hit::Button::CloseWallet)
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                return self.click_button(hit::Button::RefreshWallet)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                return self.click_button(hit::Button::Accounts)
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                return self.click_button(hit::Button::CreateRequest)
            }
            KeyCode::Char('w') | KeyCode::Char('W') => return self.toggle_wallet_daemon(),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                return self.click_button(hit::Button::NewAddress)
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                return self.click_button(hit::Button::LabelAddress)
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                return self.click_button(hit::Button::PaymentNote)
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                return self.click_button(hit::Button::Restore)
            }
            // Round 5, S8. Drawn as a chip only while there is a record; the key answers either
            // way, because a key that silently does nothing is the dead end §7a.5 exists to remove.
            KeyCode::Char('p') | KeyCode::Char('P') => return self.confirm_clear_pending(),
            KeyCode::Char('Q') => match self.selected_address() {
                Some(address) => self.modal = Some(Modal::Qr { address }),
                None => self.note("No address to show yet.", true),
            },
            KeyCode::Char('/') => {
                self.wallet.filter_open = true;
                self.wallet.filter.set_focused(true);
                self.wallet.pane = Pane::History;
            }
            KeyCode::Char('t') => return self.cycle_kind(),
            KeyCode::Enter => match self.wallet.pane {
                Pane::Subaddresses => {
                    if let Some(a) = self.selected_address() {
                        self.copy(&a);
                    }
                }
                Pane::History => {
                    self.wallet.detail = self.selected_transfer().is_some();
                    self.wallet.detail_scroll = 0;
                }
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
            KeyCode::Char('i') | KeyCode::Char('I') => {
                use fmonero_core::log_sources::LogSource;
                let location = match self.logs.source {
                    LogSource::Wallet => self
                        .ctx
                        .paths
                        .run_dir
                        .join("monero-wallet-rpc.log")
                        .display()
                        .to_string(),
                    LogSource::Node => self.ctx.settings.node.log_file.display().to_string(),
                    LogSource::WalletService => {
                        format!("User journal: {}", self.ctx.settings.wallet.unit)
                    }
                    LogSource::NodeService => {
                        format!("User journal: {}", self.ctx.settings.node.unit)
                    }
                    LogSource::Application => "Local application error state".into(),
                };
                let mut result = advanced::ResultView::complete("Log source details", vec![self.logs.source.label().into(), location, "Bounded recent snapshot. Secret-bearing lines and terminal control payloads are removed before display and search.".into()]);
                if let Some(notice) = &self.logs.notice {
                    result.lines.push(notice.clone());
                }
                self.modal = Some(Modal::Result(result));
                return Cmd::none();
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                use fmonero_core::log_sources::LogSource;
                self.logs.source = match self.logs.source {
                    LogSource::Wallet => LogSource::Node,
                    LogSource::Node => LogSource::WalletService,
                    LogSource::WalletService => LogSource::NodeService,
                    LogSource::NodeService => LogSource::Application,
                    LogSource::Application => LogSource::Wallet,
                };
                self.logs.cursor = None;
                self.logs.notice = Some("Reading selected log source…".into());
                self.logs.viewer.borrow_mut().clear();
                self.logs.empty = true;
                return self.load_log();
            }
            KeyCode::Char('/') => {
                self.logs.search_open = true;
                self.logs.search.set_focused(true);
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
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
                return self.confirm(Confirm::LogLevel {
                    from: self.logs.set_level,
                    to: next,
                });
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
        if self.send.confirming() && !self.policy_current() && k.code != KeyCode::Escape {
            return Cmd::none();
        }
        if self.send.confirming() && self.privacy() && !self.send.review_revealed {
            if matches!(k.code, KeyCode::Char('v') | KeyCode::Char('V')) {
                self.send.review_revealed = true;
            }
            if k.code != KeyCode::Escape {
                return Cmd::none();
            }
        }
        // How many accounts the wallet has, so the selector cannot name a missing one (§5).
        let accounts = self
            .status
            .as_ref()
            .and_then(|s| s.wallet.accounts)
            .unwrap_or(1);
        match self.send.on_key(k, accounts) {
            SendCmd::Sweep(request) => match self.try_start(Action::BuildSweep(request)) {
                Ok(cmd) => cmd,
                Err(_) => {
                    self.send.back_to_form();
                    Cmd::none()
                }
            },
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
        if matches!(
            &modal,
            Modal::Confirm { .. } | Modal::UseNode { .. } | Modal::Notice { .. }
        ) {
            match k.code {
                KeyCode::Down | KeyCode::PageDown => self.dialog_scroll.set(
                    self.dialog_scroll
                        .get()
                        .saturating_add(1)
                        .min(self.dialog_max.get()),
                ),
                KeyCode::Up | KeyCode::PageUp => self
                    .dialog_scroll
                    .set(self.dialog_scroll.get().saturating_sub(1)),
                _ => {}
            }
            if matches!(
                k.code,
                KeyCode::Down | KeyCode::PageDown | KeyCode::Up | KeyCode::PageUp
            ) {
                self.modal = Some(modal);
                return Cmd::none();
            }
        }
        match modal {
            Modal::Native(review) => self.native_key(review, k),
            Modal::Result(result) => self.result_key(result, k),
            Modal::Accounts(panel) => self.accounts_key(panel, k),
            Modal::AccountEditor(editor) => self.account_editor_key(editor, k),
            Modal::Workflow(form) => self.on_workflow_key(form, k),
            Modal::PaymentRequest(review) => self.on_request_key(review, k),
            Modal::Qr { address } => {
                if matches!(k.code, KeyCode::Escape | KeyCode::Enter) {
                    self.modal = self.modal_back.take().map(|m| *m);
                } else {
                    self.modal = Some(Modal::Qr { address });
                }
                Cmd::none()
            }
            Modal::Confirm { what, .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => match what {
                    Confirm::NodeUp => self.start(Action::NodeUp),
                    Confirm::NodeDown => self.start(Action::NodeDown),
                    Confirm::NodeLocal => {
                        self.picker.prev = fmonero_core::nodes::configured_address(&self.ctx);
                        self.start(Action::NodeLocal)
                    }
                    // **The same core sequence `wallet clear-pending` runs** (round 5, S8): the
                    // §2a.11 send lock first, so the record cannot be deleted between two
                    // `relay_tx` calls of a sweep, then `clear_pending_send`. One implementation
                    // reached from two surfaces, which is what §2a.3 asks for.
                    Confirm::ClearPending => self.task("clear that record", |ctx| {
                        let _lock = fmonero_core::send::take_send_lock(ctx)?;
                        fmonero_core::state::clear_pending_send(ctx)?;
                        Ok("That record is cleared. Nothing was sent or cancelled by it.".into())
                    }),
                    Confirm::LogLevel { to: level, .. } => {
                        // The daemon does not report its level, so the cycle pointer is the
                        // TUI's own and advances when the call is sent, not when it lands.
                        self.logs.level = level;
                        self.logs.set_level = Some(level);
                        self.task("change how much your node logs", move |ctx| {
                            let n = NodeClient::from_ctx(ctx)?;
                            n.mutators(ctx)?.set_log_level(level)?;
                            Ok(format!("Your node now logs at level {level}."))
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
                field,
                mut input,
            } => match k.code {
                KeyCode::Escape => Cmd::none(),
                KeyCode::Enter => {
                    let text = input.value().to_string();
                    match what {
                        Prompt::NewAddress { wallet, account } => self.start(Action::NewAddress {
                            wallet,
                            account,
                            label: text,
                        }),
                        what @ (Prompt::Label { .. } | Prompt::Note { .. }) => {
                            self.start(Action::Annotation(annotations::Job::Save {
                                what,
                                value: text,
                            }))
                        }
                    }
                }
                _ => {
                    input.handle_event(&Event::Key(k));
                    self.modal = Some(Modal::Prompt {
                        what,
                        title,
                        field,
                        input,
                    });
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
                            self.cancel_wallet_prerequisite();
                            return Cmd::none();
                        }
                        KeyCode::Enter => {
                            let name = names.get(sel).cloned().unwrap_or_default();
                            let password = pass.take();
                            return self.start(Action::WalletOpen { name, password });
                        }
                        _ => {
                            pass.handle_event(&Event::Key(k));
                            if let Some(message) = pass.error() {
                                self.note(message, true);
                            }
                        }
                    }
                } else {
                    match k.code {
                        KeyCode::Enter if names.is_empty() => {
                            self.cancel_wallet_prerequisite();
                            return Cmd::none();
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') if names.is_empty() => {
                            return self.create_wallet_modal()
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') if names.is_empty() => {
                            return self.open_restore()
                        }
                        KeyCode::Escape => {
                            self.cancel_wallet_prerequisite();
                            return Cmd::none();
                        }
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
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    self.open_tool(advanced::Tool::RecoveryBackup)
                }
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
                    mut err,
                } = *form;
                // §7a.1, revision 1: `Up`/`Down` move, `Enter` advances from every control but
                // the last where it submits, `Esc` unfocuses and then discards. This modal
                // consumes `Tab` (it holds a typed password), so without a field key the flow
                // complaint (4) asked for would be unreachable from a keyboard.
                const LAST: usize = 3;
                let refocus = |name: &mut TextInput,
                               pass: &mut SecretInput,
                               again: &mut SecretInput,
                               field: usize| {
                    name.set_focused(field == 0);
                    pass.set_focused(field == 1);
                    again.set_focused(field == 2);
                };
                match k.code {
                    KeyCode::Escape => {
                        zero(&mut pass);
                        zero(&mut again);
                        self.cancel_wallet_prerequisite();
                        return Cmd::none();
                    }
                    KeyCode::Down | KeyCode::Tab => field = (field + 1) % (LAST + 1),
                    KeyCode::Up | KeyCode::BackTab => field = (field + LAST) % (LAST + 1),
                    KeyCode::Enter if field < LAST => field += 1,
                    KeyCode::Enter => {
                        let want = name.value().trim().to_string();

                        let language = coresettings::SEED_LANGUAGES[lang].1.to_string();
                        match self.create_wallet_refusal(&want, pass.value(), again.value()) {
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
                                let password = pass.take();
                                return self.start(Action::WalletCreate {
                                    name: want,
                                    password,
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
                if let Some(message) = pass.error().or(again.error()) {
                    err = Some(message.into());
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
                "An empty password would leave the wallet unencrypted. Choose a nonempty password."
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
        // **V4: the words are in `screens::local_node_modal`**, which draws them as the overlay
        // §7a.4 specifies — three sentences, and `Dialog` renders one centred row.
        self.modal = Some(Modal::Confirm {
            what: Confirm::NodeLocal,
        });
        Cmd::none()
    }

    /// **Round 5, S10: `S` on the Node screen — set the node up.**
    ///
    /// `units::install` is exactly what `fmonero setup --units --yes` calls, and it refuses by
    /// itself inside a sandbox that has not opted in (§4.2), so this adds no new power. It is not a
    /// confirm: it writes two unit files and `monerod.conf` and starts nothing, which is the same
    /// class of act as `f` fetching a node list.
    fn setup_units(&mut self) -> Cmd<Msg> {
        if matches!(units::detect_layout(&self.ctx), Ok(units::Layout::UserUnit)) {
            self.note("Your node is already set up. Press u to start it.", false);
            return Cmd::none();
        }
        if self.remote_mode() {
            self.note(
                "You are using a public node, so there is nothing here to set up. Press l to use \
                 your own node first.",
                true,
            );
            return Cmd::none();
        }
        self.task("set your node up", |ctx| {
            units::install(ctx, false)?;
            Ok("Your node is set up. Press u to start it.".into())
        })
    }

    /// **Round 5, S8: the Wallet screen's route out of an unresolved payment record.**
    ///
    /// It is not `confirm`, for the same reason `confirm_local` is not: `confirm`'s first act is to
    /// refuse in public-node mode and to refuse when another program owns the node, and neither has
    /// anything to do with a record of the user's own payments.
    fn confirm_clear_pending(&mut self) -> Cmd<Msg> {
        if !self.has_pending() {
            self.note("Nothing is waiting to be accounted for.", false);
            return Cmd::none();
        }
        self.modal = Some(Modal::Confirm {
            what: Confirm::ClearPending,
        });
        Cmd::none()
    }

    /// Is there an unresolved payment record? `status` carries it, and a `pending.json` fmonero
    /// cannot parse still counts: "unparsable" is not "absent" (§2a.4).
    pub(super) fn has_pending(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.pending.is_some())
            || self
                .status
                .as_ref()
                .is_some_and(|s| s.warnings.iter().any(|w| w.id == "send_may_be_in_flight"))
    }

    fn confirm(&mut self, what: Confirm) -> Cmd<Msg> {
        // In public-node mode `u`, `d` and `L` say so instead of acting (§7a): all three exit 4
        // `remote_node`. **v5.6 names the two keys that do something about it** rather than the
        // shell command the refusal used to print (§2o.1) — `f` to pick another public node, `l`
        // to go back to your own. The connection change still asks first.
        if self.remote_mode() {
            // **X5: the situation first, and one refusal per key.** This was one 40-column toast
            // for three keys — `Press f for the list, or l for your own.` — which is rule 11's
            // hint standing in for the sentence, and which told a user who pressed `L` to choose a
            // node without ever saying why the log level could not be changed. W14's own fix
            // removed the width constraint that forced it: a toast too long for the status bar
            // gets a row of its own above it (`screens::note_fits`), up to two.
            self.note(
                match what {
                    // **M13: `f` gets a fresh list; it does not choose a node.** All three
                    // refusals told the reader to press `f` "for another public node", which is
                    // the third of three different descriptions of one key in one screen —
                    // choosing one also needs a selection and Enter. They name the key that does
                    // what they promise.
                    Confirm::NodeUp => {
                        "You are using a public node, so there is no node here to \
                                        start. Pick a node below and press Enter, or press l to \
                                        use your own."
                    }
                    Confirm::NodeDown => {
                        "You are using a public node, so there is no node here to \
                                          stop. Pick a node below and press Enter, or press l to \
                                          use your own."
                    }
                    // `L` is not about which node to use at all, which is why one shared refusal
                    // could never be right for it. **M13: no capitalised word** — `OWN` was the
                    // only shouted word in the product.
                    Confirm::LogLevel { .. } => {
                        "This sets how much your own node logs, and you are \
                                             using a public node. Press l to use your own node \
                                             first."
                    }
                    // `l` itself is the way back and is never refused here.
                    Confirm::NodeLocal => "Press l to use your own node.",
                    // Nor is `P`: a payment record is the wallet's, not the node's, and
                    // `confirm_clear_pending` never routes through this guard.
                    Confirm::ClearPending => "Press P on the Wallet screen.",
                },
                true,
            );
            return Cmd::none();
        }
        // **Round 5, S10: `Layout::None` is not `Layout::SystemUnit`.**
        //
        // `may_mutate_node` is `detect_layout(ctx)? == Layout::UserUnit`, so both got the same
        // refusal — and on a fresh machine in local mode, before anything has been set up, the Node
        // screen invited `u` ("Press u to start it.") and `u` answered *"Another program on this
        // machine runs this node."* Nothing ran it. Three other surfaces answered the same state
        // correctly, which is how the reader found it. And there was no in-app route to write the
        // units at all, which is the dead end §7a.5 exists to remove: `S` is that route.
        match units::detect_layout(&self.ctx) {
            Ok(units::Layout::UserUnit) => {}
            Ok(units::Layout::None) => {
                // **And "nothing is set up" is not "nothing is there"** (round 5's own read of its
                // captures). With something already answering on that port — which is what the
                // capture sandbox's mock is — `fmonero has not set up your node yet. Press S to set
                // it up.` sat under a hero reading `● Synced` and a status bar reading `node synced`.
                // Both sentences were true and a reader cannot hold them together. `Model.js`'s
                // `nodeLabel` has drawn this distinction since round 4 iteration 3; this is the same
                // two branches, in the refusal.
                let answering = self
                    .status
                    .as_ref()
                    .is_some_and(|s| s.node.reachable == Some(true));
                self.note(
                    if answering {
                        "Something else is answering on that port and fmonero did not set it up, so \
                         fmonero cannot start or stop it."
                            .to_string()
                    } else {
                        node_set_up_line()
                    },
                    true,
                );
                return Cmd::none();
            }
            _ => {
                self.note(fmonero_core::FOREIGN_NODE_READ_ONLY, true);
                return Cmd::none();
            }
        }
        // **`L` is refused while the node is not answering** (round 4 iteration 3). How much a
        // node logs is a setting the node itself holds, so with the node off the confirm was
        // guaranteed to fail — a modal, a `y`, and then a red toast. `u` and `d` are different:
        // they are the keys that change whether it runs at all.
        if matches!(what, Confirm::LogLevel { .. })
            && self
                .status
                .as_ref()
                .is_some_and(|s| s.node.reachable != Some(true))
        {
            self.note(
                format!(
                    "Your node is {}. Only a node that is running can change how much it logs. \
                     Press u to start it.",
                    self.node_off_words()
                ),
                true,
            );
            return Cmd::none();
        }
        // **All four confirms are overlays** (W2, V4, X4): each one has more than one sentence to
        // state, and `screens.rs` owns the words for every one of them.
        self.modal = Some(Modal::Confirm { what });
        Cmd::none()
    }

    /// Cancelling a prerequisite also cancels the deferred receiving action. Merely
    /// advancing from the picker to Open/Create/Restore must preserve that action.
    fn cancel_wallet_prerequisite(&mut self) {
        if matches!(
            self.launch_intent,
            LaunchIntent::NewAddress | LaunchIntent::Requests
        ) {
            self.launch_intent = LaunchIntent::Default;
        }
    }

    fn open_wallet_modal(&mut self) -> Cmd<Msg> {
        // S1: the modal lists what is there, and says why it cannot when it cannot.
        self.reload_wallets();
        let names = self.wallets.clone();
        let mut pass = SecretInput::new();
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
    fn new_address_modal(&mut self) -> Cmd<Msg> {
        if !self.status.as_ref().is_some_and(|s| s.wallet.open) {
            self.launch_intent = LaunchIntent::NewAddress;
            return self.open_wallet_modal();
        }
        self.show_new_address_prompt()
    }

    fn show_new_address_prompt(&mut self) -> Cmd<Msg> {
        self.launch_intent = LaunchIntent::Default;
        self.start(Action::NewAddressForm {
            account: self.account_index(),
        })
    }

    fn create_wallet_modal(&mut self) -> Cmd<Msg> {
        let mut name = text_input(&self.theme);
        name.set_focused(true);
        // `English` is the default, and it is index 1 of `SEED_LANGUAGES` (upstream's order).
        let lang = coresettings::SEED_LANGUAGES
            .iter()
            .position(|(_, en)| *en == coresettings::DEFAULT_SEED_LANGUAGE)
            .unwrap_or(0);
        self.modal = Some(Modal::CreateWallet(Box::new(CreateForm {
            name,
            pass: SecretInput::new(),
            again: SecretInput::new(),
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
        // **With nothing open, answer the question that was asked** (round 4 iteration 3). This
        // went straight to the wallet service, which is not running when no wallet is open, so
        // pressing `x` on an empty Wallet screen replied *"The wallet service is not answering.
        // Press o on the Wallet screen to open a wallet."* — a fact about a service he did not ask
        // about, and a route to `o`, which then answers `There are no wallets yet, in this folder:`.
        if self
            .status
            .as_ref()
            .is_some_and(|s| !s.wallet.open && s.wallet.name.is_none())
        {
            self.note("No wallet is open, so there is nothing to close.", false);
            return Cmd::none();
        }
        self.task("close the wallet", |ctx| {
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

    /// **S9: the `W` chip's label, and it is not a two-way switch while something is running.**
    ///
    /// Eight seconds into creating a wallet the chip read *"Stop the wallet service  (nothing is
    /// open, so nothing is interrupted)"* — true of the wallet handle and false of everything
    /// else in the frame, which said `Creating mywallet 8s / 300s` on the status bar and `running,
    /// but not answering yet` in the pane below. `toggle_wallet_daemon` already refused there; the
    /// label was the half that lied, and a label that describes a refusal as harmless is worse
    /// than the refusal.
    /// **`None` where pressing `W` would be refused or pointless** (round 4 iteration 3).
    ///
    /// This returned three labels and two of them told the reader not to use the chip they were
    /// written on: `Start the wallet service  (not needed: opening a wallet starts it)` and, while
    /// an action was in flight, `Stop the wallet service  (not while something is running)`. A
    /// control that advertises a route which cannot exist in the state it is drawn in is the
    /// argument M9 already settled for `[l]` and §6.5 settled for the widget's own buttons. The key
    /// stays in the keymap, where the `?` overlay explains both halves of it.
    pub(super) fn wallet_daemon_chip(&self) -> Option<&'static str> {
        if self.action.is_some() {
            return None;
        }
        self.wallet_daemon_key_stops()
            .then_some("Stop the wallet service  (nothing is open, so nothing is interrupted)")
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
            // S9: what is running and what to do, not just that something is.
            self.note(
                "Something else is running. Wait for it to finish, or press Esc to stop it.",
                true,
            );
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
        self.start(Action::Annotation(annotations::Job::LabelForm {
            account: self.account_index(),
            row: row.clone(),
        }))
    }

    fn prompt_note(&mut self) -> Cmd<Msg> {
        let Some(row) = self.selected_transfer().cloned() else {
            self.note("No payment selected.", true);
            return Cmd::none();
        };
        self.start(Action::Annotation(annotations::Job::NoteForm(row)))
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
            Screen::Wallet | Screen::Receive => self
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
            Screen::Wallet | Screen::Receive => &self.wallet.table,
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

    /// **M22: a search that says what it found.**
    ///
    /// Typing `/ Synced` highlighted the matches and left every line on screen, with nothing saying
    /// how many there were or that `n` and `N` step between them — so on three lines with one match
    /// it looked as though the search had done nothing at all. The count and the keys are the answer,
    /// and `0` is an answer too.
    fn apply_search(&mut self) -> Cmd<Msg> {
        let q = self.logs.search.value().trim().to_string();
        let found = {
            let mut log = self.logs.viewer.borrow_mut();
            if q.is_empty() {
                log.clear_search();
                None
            } else {
                log.search(&q);
                Some(log.search_info().map_or(0, |(_, total)| total))
            }
        };
        match found {
            None => self.note("Search cleared. Every line shows.", false),
            Some(0) => self.note(format!("No line has \"{q}\" in it."), true),
            Some(1) => self.note(format!("One line has \"{q}\" in it."), false),
            Some(n) => self.note(
                format!("{n} lines have \"{q}\" in them. n and N step between them."),
                false,
            ),
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
            log.push(fmonero_core::log_sources::sanitize(&format!(
                "fmonero: {line}"
            )));
        }
    }
}

/// Every input in the TUI: themed, with its hint drawn at render time by
/// `screens::hinted_input` rather than handed to ftui's own empty-field text (§2h).
fn text_input(th: &Theme) -> TextInput {
    TextInput::new()
        .with_max_length(16 * 1024)
        .with_style(th.text())
        .with_cursor_style(th.cursor())
}

fn restyle(old: &TextInput, th: &Theme) -> TextInput {
    let mut fresh = text_input(th).with_value(old.value().to_string());
    fresh.set_focused(old.focused());
    fresh
}

/// §2b.4: the password buffer is overwritten with zeroes when the modal closes.
fn zero(input: &mut SecretInput) {
    input.clear();
}

impl Model for Tui {
    type Message = Msg;

    fn init(&mut self) -> Cmd<Msg> {
        let mut cmds = vec![self.poll(), self.load_log()];
        // The picker reads the **cache** on entry (`nodes::cached`, which cannot spawn), so a fetch
        // is never on the render path and launching the TUI contacts nobody (§2n.5 rule 1, §2n.6).
        self.reload_picker();
        self.reload_wallets();
        match self.screen {
            Screen::Wallet | Screen::Receive => {
                cmds.push(self.load_wallet());
                cmds.push(self.load_history());
            }
            Screen::Node => cmds.push(self.load_extras()),
            _ => {}
        }
        if self.launch_intent == LaunchIntent::Restore {
            cmds.push(self.open_restore());
        }
        // Wallet startup on launch is opt-in; status polling never starts it.
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
                cancel: self.action.as_ref().unwrap().cancel.clone(),
            }));
        }
        v
    }

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        // Reload before keys and worker completions too. Rendering independently masks a
        // metadata mismatch, so a policy change cannot expose a stale revealed frame.
        if !matches!(&msg, Msg::Poll) {
            self.maybe_reload_settings();
        }
        match msg {
            Msg::Raw(Event::Key(k)) => self.on_key(k),
            Msg::Raw(Event::Mouse(ev)) => self.on_mouse(ev),
            Msg::Raw(mut event @ Event::Paste(_)) => {
                let cmd = self.on_paste(&event);
                if let Event::Paste(paste) = &mut event {
                    zeroize::Zeroize::zeroize(&mut paste.text);
                }
                cmd
            }
            Msg::Raw(_) => Cmd::none(),
            Msg::Poll => {
                if self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.stop_requested(&self.ctx).unwrap_or(false))
                {
                    return self.quit(0);
                }
                self.maybe_reload_theme();
                // §7a.7 part 2, before the poll that would otherwise ask `status` about the
                // node this process still thinks is configured.
                if !self.maybe_reload_settings() {
                    return Cmd::none();
                }
                let mut cmds = vec![self.poll()];
                if self.ctx.settings.tui.notifications
                    && !self.notifications_inflight
                    && self.notifications_at.elapsed() >= Duration::from_secs(15)
                    && self.action.is_none()
                    && self.status.as_ref().is_some_and(|s| s.wallet.open)
                {
                    self.notifications_inflight = true;
                    self.notifications_at = Instant::now();
                    let ctx = self.ctx.clone();
                    cmds.push(Cmd::task(move || {
                        Msg::Notifications(
                            crate::notification::poll(&ctx, &fmonero_core::CancelToken::new())
                                .map(|_| ())
                                .map_err(|e| e.to_string()),
                        )
                    }));
                }
                if self.receiving.following
                    && self.screen == Screen::Receive
                    && self.modal.is_none()
                {
                    cmds.push(self.load_receipts());
                }
                if self.logs.follow && self.screen == Screen::Log {
                    cmds.push(self.load_log());
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
            Msg::StatusRead(epoch, r) => {
                if epoch != self.read_epoch || !self.config_valid {
                    return Cmd::none();
                }
                self.update(Msg::Status(r))
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
                        self.status_revision += 1;
                        if let Some(old) = &self.status {
                            let changed_wallet = s.wallet.open
                                && (old.wallet.name != s.wallet.name
                                    || old.wallet.account_index != s.wallet.account_index
                                    || old.wallet.url != s.wallet.url
                                    || matches!((&old.wallet.primary_address, &s.wallet.primary_address),
                                    (Some(a), Some(b)) if a != b));
                            let closed_wallet = s.wallet.reachable && !s.wallet.open;
                            if changed_wallet || (closed_wallet && old.wallet.open) {
                                self.invalidate_wallet_reads();
                            }
                        }
                        let opened = s.wallet.open;
                        self.wallet.wh = s.wallet.height;
                        self.wallet.now = Some(fmonero_core::now_unix());
                        self.status_err = None;
                        self.status = Some(s);
                        if self.launch_intent == LaunchIntent::Requests
                            && self.modal.is_none()
                            && self.action.is_none()
                        {
                            if !opened {
                                self.open_wallet_modal();
                            } else if self.wallet.addresses.is_some() {
                                // Create/Restore may finish loading the wallet while their
                                // result dialog is still open. Resume after it is dismissed.
                                self.open_request(false);
                            }
                        }
                        if self.launch_intent == LaunchIntent::NewAddress
                            && self.modal.is_none()
                            && self.action.is_none()
                        {
                            if opened {
                                self.show_new_address_prompt();
                            } else {
                                self.open_wallet_modal();
                            }
                        }
                        // Load once when a wallet appears, and **not** again every second
                        // after a failure: a 1 Hz retry of four RPCs against a wallet that is
                        // answering errors is the pile-up §2d exists to prevent. `r` retries,
                        // and the pane says so.
                        if opened && self.wallet.addresses.is_none() && self.wallet.err.is_none() {
                            return Cmd::batch(vec![self.load_wallet(), self.load_history()]);
                        }
                        if !opened && !self.status.as_ref().is_some_and(|s| s.wallet.busy) {
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
            Msg::WalletData(epoch, r) => {
                if epoch != self.read_epoch {
                    return Cmd::none();
                }
                self.wallet.inflight = false;
                match *r {
                    Ok((b, a)) => {
                        self.wallet.err = None;
                        self.wallet.balance = Some(b);
                        if let Some(index) = self.wallet.select_address.take() {
                            self.wallet.table.borrow_mut().select(
                                a.addresses
                                    .iter()
                                    .position(|row| row.address_index == index),
                            );
                        }
                        self.wallet.addresses = Some(a);
                        if self.launch_intent == LaunchIntent::Requests
                            && self.modal.is_none()
                            && self.action.is_none()
                        {
                            return self.open_request(false);
                        }
                    }
                    Err(e) => self.wallet.err = Some(e),
                }
                Cmd::none()
            }
            Msg::History(epoch, seq, revision, r) => {
                if epoch != self.read_epoch || seq != self.history_seq {
                    return Cmd::none();
                }
                match *r {
                    Ok((rows, wh, now)) => {
                        self.wallet.rows_err = None;
                        self.wallet.rows = rows;
                        if revision == self.status_revision {
                            self.wallet.wh = wh;
                            self.wallet.now = now;
                        }
                        let total = self.visible_history().len();
                        let mut st = self.wallet.list.borrow_mut();
                        st.selected = (total > 0).then(|| st.selected.unwrap_or(0).min(total - 1));
                    }
                    Err(e) => self.wallet.rows_err = Some(e),
                }
                Cmd::none()
            }
            Msg::Notifications(result) => {
                self.notifications_inflight = false;
                if let Err(error) = result {
                    self.note(error, true);
                }
                Cmd::none()
            }
            Msg::Received(epoch, r) => {
                if epoch != self.read_epoch {
                    return Cmd::none();
                }
                self.receiving.inflight = false;
                match *r {
                    Ok(observed) => {
                        if self.selected_address().as_deref()
                            == Some(observed.address.address.as_str())
                        {
                            self.receiving.observed = Some(observed);
                            self.receiving.error = None;
                            if self.receiving.show_details {
                                self.receiving.show_details = false;
                                return self.receipt_details();
                            }
                        }
                    }
                    Err(error) => {
                        self.receiving.error = Some(error);
                        self.receiving.following = false;
                    }
                }
                Cmd::none()
            }
            Msg::LogSnapshot(source, r) => {
                if source != self.logs.source {
                    return Cmd::none();
                }
                match *r {
                    Ok(snapshot) => {
                        self.logs.cursor = snapshot.cursor;
                        self.logs.notice = snapshot.unavailable.or_else(|| {
                            if snapshot.rotated {
                                Some("Log rotated; showing the current file.".into())
                            } else if snapshot.truncated {
                                Some(
                                    "Older lines omitted; showing a bounded recent snapshot."
                                        .into(),
                                )
                            } else {
                                None
                            }
                        });
                        self.update(Msg::LogLines(Box::new(Ok((
                            snapshot.lines,
                            self.logs.path.clone(),
                        )))))
                    }
                    Err(why) => {
                        self.logs.notice = Some(fmonero_core::log_sources::sanitize(&why));
                        Cmd::none()
                    }
                }
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
                            log.push_many(
                                lines.iter().map(|s| fmonero_core::log_sources::sanitize(s)),
                            );
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
                        //
                        // **The prefix is a sentence, not a bare internal label** (round 4
                        // iteration 3). It read `close: The wallet service is not answering.` and
                        // `label: …` — and `label` is a word the interface never shows a user, where
                        // every chip and help row says *rename*. `R`'s own failure carried no prefix
                        // at all, so two frames in one session named the same failure two ways. Each
                        // `task` now passes the verb phrase that completes "fmonero could not …",
                        // and the one that had none has one.
                        self.note(format!("fmonero could not {label}. {}", e.toast()), true);
                    }
                }
                Cmd::none()
            }
            Msg::ActionFinished(id, r) => {
                if self.action.as_ref().is_none_or(|a| a.id != id) {
                    return Cmd::none();
                }
                self.update(Msg::ActionDone(r))
            }
            Msg::ActionDone(r) => {
                self.action = None;
                self.quit_armed = None;
                match *r {
                    Ok(Outcome::Native(reply)) => match reply {
                        native::Reply::Held(review) => self.modal = Some(Modal::Native(review)),
                        native::Reply::Result(result) => {
                            let _ = self.reload_ctx();
                            self.reload_wallets();
                            self.invalidate_wallet_reads();
                            self.note(result.title.clone(), result.failed);
                            self.modal = Some(Modal::Result(result));
                        }
                    },
                    Ok(Outcome::AnnotationForm {
                        what,
                        title,
                        field,
                        value,
                    }) => {
                        self.modal = Some(Modal::Prompt {
                            what,
                            title,
                            field,
                            input: text_input(&self.theme).with_value(value).with_focused(true),
                        });
                    }
                    Ok(Outcome::ToolReady(tool, identity)) => {
                        self.modal = Some(Modal::Workflow(workflows::Form::tool(
                            self,
                            tool,
                            Some(identity),
                        )));
                    }
                    Ok(Outcome::Advanced(result)) => {
                        let _ = self.reload_ctx();
                        self.reload_wallets();
                        self.invalidate_wallet_reads();
                        self.note(
                            if result.failed {
                                "Operation requires attention."
                            } else {
                                "Operation complete."
                            },
                            result.failed,
                        );
                        self.modal = Some(Modal::Result(result));
                    }
                    Ok(Outcome::Accounts(reply)) => {
                        if let Err(error) = self.reload_ctx() {
                            self.note(error.to_string(), true);
                        }
                        self.invalidate_wallet_reads();
                        if !reply.message.is_empty() {
                            self.note(reply.message, false);
                        }
                        if let Some(snapshot) = reply.snapshot {
                            self.modal = Some(Modal::Accounts(accounts::Panel {
                                snapshot,
                                contacts: reply.contacts,
                                selected: 0,
                            }));
                        }
                    }
                    Ok(Outcome::Note(text)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(text, false);
                    }
                    Ok(Outcome::NewAddressForm { wallet, account }) => {
                        self.wallet.new_address_identity =
                            Some((wallet.clone(), account, self.read_epoch));
                        self.modal = Some(Modal::Prompt {
                            what: Prompt::NewAddress { wallet, account },
                            title: " New receiving address ".into(),
                            field: "Label (optional) ".into(),
                            input: text_input(&self.theme).with_focused(true),
                        });
                    }
                    Ok(Outcome::AddressCreated(address)) => {
                        let same = self.wallet.new_address_identity.take().is_some_and(
                            |(wallet, account, epoch)| {
                                wallet == address.wallet
                                    && account == address.account_index
                                    && epoch == self.read_epoch
                                    && self.ctx.settings.wallet.account == account
                                    && self.status.as_ref().is_some_and(|s| {
                                        s.wallet.open
                                            && s.wallet.name.as_deref()
                                                == Some(wallet.name.as_str())
                                    })
                            },
                        );
                        if same {
                            self.invalidate_wallet_reads();
                            self.wallet.select_address = Some(address.address_index);
                            self.wallet.pane = Pane::Subaddresses;
                            self.screen = Screen::Receive;
                            self.tabs.borrow_mut().active = Screen::Receive.index();
                        }
                        self.note(
                            format!(
                                "Address #{} created in account {}.",
                                address.address_index, address.account_index
                            ),
                            false,
                        );
                    }
                    Ok(Outcome::Opened(name)) => {
                        let _ = self.reload_ctx();
                        self.invalidate_wallet_reads();
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(format!("{name} is open."), false);
                        if self.launch_intent == LaunchIntent::NewAddress {
                            self.show_new_address_prompt();
                        }
                    }
                    Ok(Outcome::Built(plan)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        // M19: what was done, in the words the form and the Preview use.
                        // One wording for this on all four surfaces (round 4 iteration 3).
                        self.note("The fee is worked out. Nothing has been sent yet.", false);
                        self.send.typed.clear();
                        self.send.tab_note = false;
                        self.send.reset_review();
                        self.send.stage = Stage::Built(plan);
                        return Cmd::none();
                    }
                    Ok(Outcome::Relayed(txids)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        // S10: no id in a toast. The screen it jumps to is the answer, and
                        // `txids` is what `--json` carries for an agent.
                        let _ = &txids;
                        self.note("Sent. The payment is at the top of your payments.", false);
                        self.send.clear();
                        self.send.stage = Stage::Sent;
                        self.screen = Screen::Wallet;
                        self.tabs.borrow_mut().active = Screen::Wallet.index();
                        self.wallet.pane = Pane::History;
                        self.wallet.list.borrow_mut().selected = Some(0);
                    }
                    // ---- v5.5 -------------------------------------------------------
                    Ok(Outcome::Restored(result)) => {
                        let _ = self.reload_ctx();
                        self.invalidate_wallet_reads();
                        self.reload_wallets();
                        let result = advanced::ResultView::restored(result);
                        self.note(result.title.clone(), result.failed);
                        self.modal = Some(Modal::Result(result));
                    }
                    Ok(Outcome::Created { name }) => {
                        let _ = self.reload_ctx();
                        self.invalidate_wallet_reads();
                        self.exit.store(0, Ordering::SeqCst);
                        self.note(format!("{name} created and open."), false);
                        let mut lines = vec![
                            "Before receiving funds, create an encrypted recovery wallet.".into(),
                            "Choose a new private destination and a recovery password. Monero generates the artifact and verifies its address and password before publication.".into(),
                            "This recovers funds. Labels, contacts, notes and history metadata are not included.".into(),
                            "Recovery words and private keys never appear on screen.".into(),
                        ];
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
                        self.reload_wallets();
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
                            // X14: the third un-pluralised toast, after the self-read fixed two.
                            format!(
                                "Checked {} {}. {reached} answered.",
                                pairs.len(),
                                if pairs.len() == 1 { "node" } else { "nodes" }
                            ),
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
