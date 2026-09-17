//! The one cancellable background worker (DESIGN.md §7a).
//!
//! Long actions — node up/down, open wallet, wallet refresh, and the two halves of a send —
//! never run in a `Cmd::task`, because a `Cmd` cannot be cancelled. They run in
//! [`ActionSub`]: the model holds it only while the action is in flight, so dropping it
//! makes the runtime raise the `StopSignal`, which trips the core `CancelToken`, which
//! returns `Error::Interrupted`.
//!
//! [`Action`] is **not** `Clone` — it carries a wallet password and a [`send::Plan`] that
//! owns the §2a.11 `send.lock` — while `Subscription::run` takes `&self`
//! (`ftui-runtime-0.7.0/src/subscription.rs:46`). So the action lives behind interior
//! mutability and is moved out exactly once (§7a): `subscriptions()` clones the `Arc`, never
//! the `Action`, and `run` `take()`s it before consuming it. That is also what makes "the
//! variant is dropped and its `String` zeroed as soon as the subscription takes it" true.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ftui::runtime::{StopSignal, SubId, Subscription};

use fmonero_core::{
    node_session, send, wallet_session, CancelToken, Context, Error, SendRequest, WalletClient,
};

use super::Msg;

/// Everything the TUI can start that is allowed to take a minute (§7a).
///
/// **Derives neither `Debug` nor `Clone`**: a derived `Clone` leaves a second heap copy of
/// the password nobody zeroes, and a derived `Debug` puts it into any panic message. The
/// `Debug` below is hand-written and prints the password as `<redacted>`, exactly as §5b's
/// `BuiltTx::metadata` does.
pub enum Action {
    /// → `node_session::up` (§5b), never `units::start`: the §2i single-writer precondition
    /// exists in exactly one place and the TUI's copy would be the one nobody reviews.
    NodeUp,
    /// → `node_session::down`.
    NodeDown,
    /// → `wallet_session::open`, so the modal cannot omit or reorder the `set_daemon` step
    /// whose absence means the wallet can never sync (§4.2).
    WalletOpen {
        name: String,
        password: String,
    },
    WalletRefresh {
        start_height: u64,
    },
    /// → `send::build` (§5b). The TUI never calls `build_transfer` itself (§2a.3).
    BuildTx(SendRequest),
    /// → `send::relay`, with the **same** plan that was displayed (§2a.3).
    Relay(send::Plan),
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::NodeUp => f.write_str("NodeUp"),
            Action::NodeDown => f.write_str("NodeDown"),
            Action::WalletOpen { name, .. } => f
                .debug_struct("WalletOpen")
                .field("name", name)
                .field("password", &"<redacted>")
                .finish(),
            Action::WalletRefresh { start_height } => f
                .debug_struct("WalletRefresh")
                .field("start_height", start_height)
                .finish(),
            Action::BuildTx(_) => f.write_str("BuildTx"),
            Action::Relay(_) => f.write_str("Relay"),
        }
    }
}

impl Action {
    /// The label the status line shows while this runs.
    pub fn label(&self) -> String {
        match self {
            Action::NodeUp => "starting the node".into(),
            Action::NodeDown => "stopping the node".into(),
            Action::WalletOpen { name, .. } => format!("opening {name}"),
            Action::WalletRefresh { .. } => "refreshing the wallet".into(),
            Action::BuildTx(_) => "building the transaction".into(),
            Action::Relay(_) => "broadcasting".into(),
        }
    }

    /// The deadline the status line counts against, so `12s / 120s` is honest.
    pub fn deadline_sec(&self, ctx: &Context) -> u64 {
        match self {
            Action::NodeUp | Action::NodeDown => ctx.timeouts.unit_probe.as_secs(),
            Action::WalletOpen { .. } => ctx.timeouts.open.as_secs(),
            Action::WalletRefresh { .. } => ctx.timeouts.refresh.as_secs(),
            Action::BuildTx(_) => ctx.timeouts.transfer.as_secs(),
            Action::Relay(_) => ctx.timeouts.relay.as_secs(),
        }
    }

    /// `Action::Relay` **ignores the StopSignal** (§7a): once `relay_tx` is on the wire,
    /// cancelling tells you nothing about whether the node got it, and the `pending.json`
    /// record is the mechanism that covers it.
    pub fn cancellable(&self) -> bool {
        !matches!(self, Action::Relay(_))
    }
}

/// What an action produced, in the form the model needs to go on.
pub enum Outcome {
    /// A note for the status line (node up/down, wallet refresh).
    Note(String),
    /// The wallet that was opened, so the model can reload its screens.
    Opened(String),
    /// A built, unbroadcast plan — the one the confirm modal renders and `Relay` consumes.
    Built(Box<send::Plan>),
    /// The relayed txids, in relay order.
    Relayed(Vec<String>),
}

/// A failed action, reduced to what the TUI shows and exits with (§4a).
#[derive(Clone, Debug)]
pub struct ActionErr {
    pub code: i32,
    pub code_name: String,
    pub text: String,
    pub hint: String,
    pub interrupted: bool,
}

impl From<Error> for ActionErr {
    fn from(e: Error) -> ActionErr {
        ActionErr {
            code: e.exit_code(),
            code_name: e.code_name().to_string(),
            text: e.to_string(),
            hint: e.hint(),
            interrupted: e.is_interrupted(),
        }
    }
}

/// The body of [`ActionSub::run`], as a plain function so the §7a `ProgramSimulator` test
/// drives the **same** code path (`StopSignal` has no public constructor, so a test cannot
/// call `run` itself).
///
/// `announce` is `send::relay`'s sink (§5b): core never prints (§2g), so the line naming the
/// recipient, amount, fee, total, priority and account is appended to the Log screen's pane
/// here — the CLI writes the same line to stderr.
pub fn execute(
    ctx: &Context,
    action: Action,
    announce: &Arc<Mutex<Vec<String>>>,
    cancel: &CancelToken,
) -> Result<Outcome, ActionErr> {
    let sink = announce.clone();
    let say = move |s: &str| {
        if let Ok(mut v) = sink.lock() {
            v.push(s.to_string());
        }
    };
    let run = || -> Result<Outcome, Error> {
        match action {
            Action::NodeUp => {
                let s = node_session::up(ctx, None, cancel)?;
                Ok(Outcome::Note(if s.already_running {
                    "the node was already running".into()
                } else {
                    format!("node up in {}s", s.waited_sec)
                }))
            }
            Action::NodeDown => {
                node_session::down(ctx, cancel)?;
                Ok(Outcome::Note("node stopped".into()))
            }
            Action::WalletOpen { name, password } => {
                let w = WalletClient::from_ctx(ctx)?;
                let opened = wallet_session::open(ctx, &w, &name, password, cancel)?;
                Ok(Outcome::Opened(opened.name))
            }
            Action::WalletRefresh { start_height } => {
                let w = WalletClient::from_ctx(ctx)?;
                let r = w.refresh(start_height, cancel)?;
                Ok(Outcome::Note(format!(
                    "refreshed {} blocks{}",
                    r.blocks_fetched,
                    if r.received_money {
                        " · received money"
                    } else {
                        ""
                    }
                )))
            }
            Action::BuildTx(req) => {
                let w = WalletClient::from_ctx(ctx)?;
                let n = fmonero_core::NodeClient::from_ctx(ctx)?;
                let plan = send::build(ctx, &w, Some(&n), &req, cancel)?;
                Ok(Outcome::Built(Box::new(plan)))
            }
            Action::Relay(plan) => {
                let w = WalletClient::from_ctx(ctx)?;
                let txids = send::relay(ctx, &w, plan, &say, cancel)?;
                Ok(Outcome::Relayed(txids))
            }
        }
    };
    run().map_err(ActionErr::from)
}

pub struct ActionSub {
    pub id: SubId,
    pub ctx: Context,
    /// The slot the model owns; `run` moves the action out of it exactly once (§7a).
    pub slot: Arc<Mutex<Option<Action>>>,
    pub announce: Arc<Mutex<Vec<String>>>,
}

impl Subscription<Msg> for ActionSub {
    fn id(&self) -> SubId {
        self.id
    }

    fn run(&self, tx: Sender<Msg>, stop: StopSignal) {
        let Some(action) = self.slot.lock().ok().and_then(|mut s| s.take()) else {
            return; // an empty slot: the runtime restarted an unchanged subscription
        };
        let cancel = CancelToken::new();
        let done = Arc::new(AtomicBool::new(false));
        let watcher = if action.cancellable() {
            let watched = cancel.clone();
            let watcher_done = done.clone();
            // The runtime has no "cancel me" callback: it drops the subscription and raises
            // the stop signal, so one thread translates that into the CancelToken core polls.
            Some(std::thread::spawn(move || {
                while !watcher_done.load(Ordering::Relaxed) {
                    if stop.wait_timeout(Duration::from_millis(200)) {
                        watched.cancel();
                        return;
                    }
                }
            }))
        } else {
            None
        };

        let outcome = execute(&self.ctx, action, &self.announce, &cancel);
        done.store(true, Ordering::Relaxed);
        if let Some(w) = watcher {
            let _ = w.join();
        }
        let _ = tx.send(Msg::ActionDone(Box::new(outcome)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debugging_an_open_never_prints_the_password() {
        let a = Action::WalletOpen {
            name: "w".into(),
            password: "hunter2".into(),
        };
        let shown = format!("{a:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains('w'), "{shown}");
    }

    #[test]
    fn every_action_but_a_relay_is_cancellable() {
        assert!(Action::NodeUp.cancellable());
        assert!(Action::NodeDown.cancellable());
        assert!(Action::WalletRefresh { start_height: 0 }.cancellable());
        // `Action::Relay` cannot be constructed without a real built `Plan` (its `send.lock`
        // file descriptor is private to core), so its arm is asserted in the send module's
        // `ProgramSimulator` test, which builds one against the mock.
    }

    #[test]
    fn an_error_keeps_its_own_exit_code_and_hint() {
        let e: ActionErr = Error::wallet_locked("no wallet open").into();
        assert_eq!(e.code, 9);
        assert_eq!(e.code_name, "wallet_locked");
        assert!(!e.hint.is_empty());
        assert!(!e.interrupted);
        let i: ActionErr = Error::Interrupted.into();
        assert_eq!(i.code, 130);
        assert!(i.interrupted);
    }

    #[test]
    fn labels_name_what_is_running() {
        assert_eq!(Action::NodeUp.label(), "starting the node");
        assert_eq!(
            Action::WalletOpen {
                name: "main".into(),
                password: String::new()
            }
            .label(),
            "opening main"
        );
    }
}
