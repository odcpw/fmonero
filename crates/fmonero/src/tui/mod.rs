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
use ftui::{App, Cmd, Event, KeyCode, KeyEvent, KeyEventKind, Model, Modifiers, Style};

use fmonero_core::rpc::node::{Ban, Connection, TxpoolStats};
use fmonero_core::rpc::wallet::{AddressSet, Balance, BookEntry};
use fmonero_core::{
    history, log as corelog, proc, state, status as corestatus, units, wallet_dir, Context, Error,
    HistoryQuery, NodeClient, SendRequest, Status, Transfer, TransferKind, WalletClient,
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
    (
        "q  Ctrl+C",
        "quit (twice while an action runs: cancel, then quit)",
    ),
    ("?", "this help"),
    ("Tab  ⇧Tab  1 2 3 4", "Node · Wallet · Send · Log"),
    ("r", "refresh now"),
    ("Esc", "close this overlay, else cancel the running action"),
    ("u  d", "Node: start · stop the node (confirm modal)"),
    ("b", "Node: peers ⇄ bans"),
    ("L", "cycle the daemon log level (confirm modal)"),
    ("o  c", "Wallet: open (modal) · close"),
    ("a  l", "Wallet: new subaddress · label the selected one"),
    ("n", "Wallet: edit the selected transfer's note"),
    ("R  Q", "Wallet: refresh · show the address as a QR"),
    ("/  t", "Wallet: filter rows · cycle the type filter"),
    (
        "←→  Enter",
        "Wallet: swap panes · copy an address · open a row",
    ),
    (
        "Send: Tab  Enter",
        "next field · build  (Esc leaves the form, Enter returns to it)",
    ),
    (
        "Send: p  Ctrl+P  @",
        "cycle priority (on its row, or from anywhere) · address book",
    ),
    (
        "Send: ←→",
        "on the Priority or Account row: change it (accounts come from the wallet)",
    ),
    (
        "Send: sweep",
        "not in the TUI — run: fmonero wallet sweep <addr> --dry-run",
    ),
    (
        "Send: type \"send\"",
        "the confirm modal's --yes; Esc discards, and a relay cannot be cancelled",
    ),
    ("Log: /  f  n N", "search · follow · next/previous match"),
];

const SPINNER: [&str; 4] = ["⠋", "⠙", "⠸", "⠴"];
/// One sample per `status` poll; §7a asks for a 60-sample rx sparkline.
const RX_SAMPLES: usize = 60;
/// How many history rows the Wallet screen asks core for.
const HISTORY_LIMIT: usize = 500;
/// Lines of the daemon log the Log screen tails.
const LOG_LINES: usize = 2_000;
pub(super) const FILTER_HINT: &str = "filter by txid, address or note";
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
}

#[derive(Clone, Copy)]
pub(super) enum Confirm {
    NodeUp,
    NodeDown,
    /// The next level in the 0–4 cycle. A mutation, so it asks first (§7a).
    LogLevel(i8),
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
    screen: Screen,
    tabs: RefCell<TabsState>,
    show_help: bool,
    tick: usize,
    status: Option<Status>,
    status_err: Option<String>,
    poll_inflight: bool,
    node: NodeUi,
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
}

/// Run the TUI. `Ok(0)` on a clean quit, the §4a code of the session's last failed action
/// otherwise, `Ok(130)` on Ctrl+C — the TUI never invents an exit code (§7a).
pub fn run(args: TuiArgs, ctx: Context) -> anyhow::Result<i32> {
    let exit = Arc::new(AtomicI32::new(0));
    let app = Tui::new(args.screen(), ctx, exit.clone());
    App::fullscreen(app).run()?;
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
            node: NodeUi::default(),
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
        }
    }

    // ------------------------------------------------------------ what the screens ask

    pub(super) fn account_index(&self) -> u32 {
        self.status
            .as_ref()
            .map_or(self.ctx.settings.wallet.account, |s| s.wallet.account_index)
    }

    pub(super) fn node_badge(&self) -> &'static str {
        let Some(s) = &self.status else { return "down" };
        if self.status_err.is_some() || s.pending.is_some() || self.has_warning("node_stalled") {
            return "error";
        }
        match (s.node.reachable, s.node.synchronized) {
            (false, _) => "down",
            (true, Some(true)) => "up",
            (true, _) => "syncing",
        }
    }

    pub(super) fn wallet_badge(&self) -> &'static str {
        let Some(s) = &self.status else { return "down" };
        if self.has_warning("wallet_stalled") {
            return "error";
        }
        match (s.wallet.reachable, s.wallet.open) {
            (false, _) => "down",
            (true, true) => "up",
            (true, false) => "syncing",
        }
    }

    pub(super) fn node_phrase(&self) -> String {
        let Some(s) = &self.status else {
            return "reading status…".into();
        };
        if !s.node.reachable {
            return "Node is not running".into();
        }
        match (s.node.synchronized, s.node.sync_percent) {
            (Some(true), _) => "Synchronized".into(),
            (_, Some(p)) => format!("Syncing — {p:.1}%"),
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
            .map(|w| format!("{} — {}", w.text, w.hint))
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

    /// `start`, but it hands the action **back** when it refuses.
    ///
    /// Both guards below consume the `Action` and send no `Msg::ActionDone`, so a caller that
    /// has already advanced the §7a send state machine — `SendCmd::Relay` replaces `Built` with
    /// `Relaying`, `Msg::Validated` replaces `Form` with `Building` — would be left in a stage
    /// only `ActionDone` can leave, with the built `Plan` (and its §2a.11 lock) dropped and
    /// nothing on screen saying a transaction had existed. Those two callers put it back.
    fn try_start(&mut self, action: Action) -> Result<Cmd<Msg>, Box<Action>> {
        if let Some(a) = &self.action {
            self.note(format!("{} already — Esc cancels it", a.label), true);
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
            started: Instant::now(),
            slot: Arc::new(Mutex::new(Some(action))),
        });
        Ok(Cmd::none())
    }

    /// `Esc`, or `q` twice: drop the subscription, which raises the `StopSignal`, which trips
    /// the `CancelToken`. A relay is never cancelled (§7a).
    fn cancel_action(&mut self) -> Cmd<Msg> {
        match self.action.take() {
            None => self.note("nothing running to cancel", false),
            Some(a) if !a.cancellable => {
                let label = a.label.clone();
                self.action = Some(a);
                self.note(
                    format!("{label} cannot be cancelled — the transaction is already on the wire"),
                    true,
                );
            }
            Some(a) => self.note(format!("cancelled: {}", a.label), true),
        }
        Cmd::none()
    }

    fn note(&mut self, text: impl Into<String>, err: bool) {
        self.note = text.into();
        self.note_err = err;
        self.note_at = Instant::now();
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
            Screen::Node if self.node.extras.is_none() => self.load_extras(),
            Screen::Wallet if self.wallet.addresses.is_none() => {
                Cmd::batch(vec![self.load_wallet(), self.load_history()])
            }
            Screen::Log if self.logs.empty => self.load_log(),
            _ => Cmd::none(),
        }
    }

    /// The clipboard is `wl-copy`, as argv (§2f) — never a shell string.
    fn copy(&mut self, text: &str) {
        if proc::which("wl-copy").is_none() {
            self.note("wl-copy is not installed — cannot copy", true);
            return;
        }
        match proc::run_with_deadline(&["wl-copy", "--", text], Duration::from_secs(2), None) {
            Ok(o) if o.success() => self.note("copied to the clipboard", false),
            Ok(o) => self.note(format!("wl-copy failed: {}", o.stderr_tail()), true),
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
                    "a broadcast is on the wire; press quit again to leave it running"
                };
                self.note(format!("an action is running — {extra}"), true);
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
            KeyCode::BackTab => {
                return self.set_screen(Screen::from_index(self.screen.index() + 3))
            }
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
                // The form is unfocused: Enter returns to it, Tab moves on.
                match k.code {
                    KeyCode::Enter => {
                        self.send.resume();
                        Cmd::none()
                    }
                    KeyCode::Tab => self.set_screen(Screen::from_index(self.screen.index() + 1)),
                    _ => Cmd::none(),
                }
            }
        }
    }

    fn on_node_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Tab => return self.set_screen(Screen::from_index(self.screen.index() + 1)),
            KeyCode::Char('u') => return self.confirm(Confirm::NodeUp),
            KeyCode::Char('d') => return self.confirm(Confirm::NodeDown),
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
            KeyCode::Down | KeyCode::Char('j') => self.move_table(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_table(-1),
            _ => {}
        }
        Cmd::none()
    }

    fn on_wallet_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            // KEYS says `Tab` is the next screen on every screen and `←→` swaps the Wallet's
            // panes — and it is also the `?` overlay, so both halves have to be true. `Tab` used
            // to swap panes here, which made the Wallet the one screen `Tab` could not leave,
            // and `←→` did nothing at all.
            KeyCode::Tab => return self.set_screen(Screen::from_index(self.screen.index() + 1)),
            KeyCode::Left | KeyCode::Right => {
                self.wallet.pane = match self.wallet.pane {
                    Pane::Subaddresses => Pane::History,
                    Pane::History => Pane::Subaddresses,
                };
            }
            KeyCode::Char('o') => return self.open_wallet_modal(),
            KeyCode::Char('c') => {
                return self.task("close", |ctx| {
                    let w = WalletClient::from_ctx(ctx)?;
                    fmonero_core::wallet_session::close(ctx, &w)?;
                    Ok("wallet closed".into())
                })
            }
            KeyCode::Char('a') => {
                let account = self.account_index();
                return self.task("new address", move |ctx| {
                    let w = WalletClient::from_ctx(ctx)?;
                    let a = w.create_address(account, "")?;
                    Ok(format!("new subaddress {}", a.address_index))
                });
            }
            KeyCode::Char('l') => return self.prompt_label(),
            KeyCode::Char('n') => return self.prompt_note(),
            KeyCode::Char('R') => {
                return self.start(Action::WalletRefresh { start_height: 0 });
            }
            KeyCode::Char('Q') => match self.selected_address() {
                Some(address) => self.modal = Some(Modal::Qr { address }),
                None => self.note("no address to show yet", true),
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
            KeyCode::Tab => return self.set_screen(Screen::from_index(self.screen.index() + 1)),
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
                self.note(if on { "following" } else { "follow off" }, false);
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
                    Confirm::LogLevel(level) => {
                        // The daemon does not report its level, so the cycle pointer is the
                        // TUI's own and advances when the call is sent, not when it lands.
                        self.logs.level = level;
                        self.task("log level", move |ctx| {
                            let n = NodeClient::from_ctx(ctx)?;
                            n.mutators(ctx)?.set_log_level(level)?;
                            Ok(format!("daemon log level {level}"))
                        })
                    }
                },
                _ => {
                    self.note("cancelled", false);
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
                                Ok("label saved".into())
                            })
                        }
                        Prompt::Note { txid } => self.task("note", move |ctx| {
                            let w = WalletClient::from_ctx(ctx)?;
                            w.set_tx_notes(&[txid], &[text])?;
                            Ok("note saved".into())
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
        }
    }

    // ------------------------------------------------------------ modal builders

    fn confirm(&mut self, what: Confirm) -> Cmd<Msg> {
        if !matches!(units::may_mutate_node(&self.ctx), Ok(true)) {
            self.note(
                "this monerod is not fmonero's to mutate — sudo systemctl <verb> monerod",
                true,
            );
            return Cmd::none();
        }
        let (title, message) = match what {
            Confirm::NodeUp => (" Start the node ", "systemctl --user start · y confirms"),
            Confirm::NodeDown => (
                " Stop the node ",
                "stop_daemon then systemctl --user stop · y confirms",
            ),
            Confirm::LogLevel(l) => (
                " Daemon log level ",
                match l {
                    0 => "set the daemon log level to 0 · y confirms",
                    1 => "set the daemon log level to 1 · y confirms",
                    2 => "set the daemon log level to 2 · y confirms",
                    3 => "set the daemon log level to 3 · y confirms",
                    _ => "set the daemon log level to 4 · y confirms",
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

    fn prompt_label(&mut self) -> Cmd<Msg> {
        let Some(set) = &self.wallet.addresses else {
            self.note("no subaddress selected", true);
            return Cmd::none();
        };
        let i = self.wallet.table.borrow().selected.unwrap_or(0);
        let Some(row) = set.addresses.get(i) else {
            self.note("no subaddress selected", true);
            return Cmd::none();
        };
        self.modal = Some(Modal::Prompt {
            what: Prompt::Label {
                index: row.address_index,
            },
            title: format!(" Label subaddress {} ", row.address_index),
            input: text_input(&self.theme)
                .with_value(row.label.clone())
                .with_focused(true),
        });
        Cmd::none()
    }

    fn prompt_note(&mut self) -> Cmd<Msg> {
        let Some(t) = self.selected_transfer() else {
            self.note("no transfer selected", true);
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
        match self.screen {
            Screen::Wallet => {
                cmds.push(self.load_wallet());
                cmds.push(self.load_history());
            }
            Screen::Node => cmds.push(self.load_extras()),
            _ => {}
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
            Msg::Raw(_) => Cmd::none(),
            Msg::Poll => {
                self.maybe_reload_theme();
                let mut cmds = vec![self.poll()];
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
                        self.note(format!("{label}: {} ({})", e.text, e.code_name), true);
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
                        self.note(format!("{name} is open"), false);
                    }
                    Ok(Outcome::Built(plan)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        self.note("built — nothing is broadcast yet", false);
                        self.send.typed.clear();
                        self.send.stage = Stage::Built(plan);
                        return Cmd::none();
                    }
                    Ok(Outcome::Relayed(txids)) => {
                        self.exit.store(0, Ordering::SeqCst);
                        let first = txids.first().cloned().unwrap_or_default();
                        self.note(format!("sent · {first}"), false);
                        self.send.clear();
                        self.send.stage = Stage::Sent(first);
                        self.screen = Screen::Wallet;
                        self.tabs.borrow_mut().active = Screen::Wallet.index();
                        self.wallet.pane = Pane::History;
                        self.wallet.list.borrow_mut().selected = Some(0);
                    }
                    Err(e) => {
                        self.exit.store(e.code, Ordering::SeqCst);
                        if matches!(self.send.stage, Stage::Building | Stage::Relaying) {
                            self.send.back_to_form();
                        }
                        if e.interrupted {
                            self.note("cancelled", true);
                        } else {
                            self.note(format!("{} ({}) — {}", e.text, e.code_name, e.hint), true);
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
