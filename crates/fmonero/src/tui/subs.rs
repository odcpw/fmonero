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

use fmonero_core::nodes::{self, NodeList, Probe, ProbeTarget};
use fmonero_core::{
    node_mode, node_session, send, units, wallet_session, CancelToken, Context, Error, SendRequest,
    WalletClient,
};

use super::Msg;

/// Everything the TUI can start that is allowed to take a minute (§7a).
///
/// **Derives neither `Debug` nor `Clone`**: a derived `Clone` leaves a second heap copy of
/// the password nobody zeroes, and a derived `Debug` puts it into any panic message. The
/// `Debug` below is hand-written and prints the password as `<redacted>`, exactly as §5b's
/// `BuiltTx::metadata` does.
pub(super) enum Action {
    Annotation(super::annotations::Job),
    Native(Box<super::native::Job>),
    PrepareTool(super::advanced::Tool),
    Advanced(super::advanced::Job),
    Accounts(super::accounts::Job),
    NewAddressForm {
        account: u32,
    },
    NewAddress {
        wallet: fmonero_core::wallet_ops::WalletIdentity,
        account: u32,
        label: String,
    },
    Restore(fmonero_core::restore::RestoreRequest),
    /// → `node_session::up` (§5b), never `units::start`: the §2i single-writer precondition
    /// exists in exactly one place and the TUI's copy would be the one nobody reviews.
    NodeUp,
    /// → `node_session::down`.
    NodeDown,
    /// → `wallet_session::open`, so the modal cannot omit or reorder the `set_daemon` step
    /// whose absence means the wallet can never sync (§4.2).
    WalletOpen {
        name: String,
        password: fmonero_core::SecretString,
    },
    WalletRefresh {
        start_height: u64,
    },
    /// → `send::build` (§5b). The TUI never calls `build_transfer` itself (§2a.3).
    BuildTx(SendRequest),
    BuildSweep(send::SweepRequest),
    /// → `send::relay`, with the **same** plan that was displayed (§2a.3).
    Relay(send::Plan),
    // ---- v5.5 -----------------------------------------------------------------
    /// → `units::ensure_running` + `create_wallet` + the last three steps of
    /// `wallet_session::open` on the wallet `create_wallet` left open (§4.1, §7a.5). The TUI
    /// adds no step and skips none. Carries a password, so the hand-written `Debug` below
    /// redacts it and the `String` is zeroed when the subscription takes it, exactly as
    /// `WalletOpen`'s is.
    WalletCreate {
        name: String,
        password: fmonero_core::SecretString,
        language: String,
    },
    /// → `units::ensure_running(wallet.unit)` (§7a.5) — the same function `wallet open` uses,
    /// so it probes first, then runs §2i's preflight, and only then starts.
    WalletDaemonStart,
    /// → `units::stop(wallet.unit)`. Refused by the model unless the unit is active, nothing is
    /// open and nothing is in flight: stopping a daemon holding an open wallet is a thing the
    /// user will not have meant.
    WalletDaemonStop,
    /// → `nodes::list` (§2n) — spawns `curl`, bounded, cancellable.
    NodeListFetch {
        refresh: bool,
    },
    /// → `nodes::probe_many` (§2n.4), 8 concurrent, cancellable. **Targets and never URLs**, so
    /// monero.fail's own bytes cannot reach a `curl` argv (§2n.1, revision 2).
    NodeProbe {
        targets: Vec<ProbeTarget>,
    },
    /// → `node_mode::set_remote(ctx, &address, trusted, false, cancel)` — the identical core
    /// call `fmonero node remote <addr> --yes` makes, so the `send.lock`, the `pending_send`
    /// check, the single `Settings::save`, the unit render and the conditional restart all
    /// happen exactly once and in one implementation (§2a.3, §2m.5).
    NodeUseRemote {
        address: String,
        trusted: bool,
    },
    /// v5.6, §7a.4's `l` → `node_mode::set_local(ctx, false, cancel)`, the identical core call
    /// `fmonero node local --yes` makes. It exists because §2o.1 forbids the TUI from telling a
    /// user to run a command: the refusal for `u`, `d` and `L` in public-node mode used to print
    /// `fmonero node local --yes`, and a rule that bans the command needs somewhere for it to go.
    NodeLocal,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::Native(_) => f.write_str("Native(<protected>)"),
            Action::Annotation(_) => f.write_str("Annotation"),
            Action::PrepareTool(_) => f.write_str("PrepareTool"),
            Action::Advanced(_) => f.write_str("Advanced"),
            Action::Accounts(_) => f.write_str("Accounts"),
            Action::NewAddressForm { .. } => f.write_str("NewAddressForm"),
            Action::NewAddress { .. } => f.write_str("NewAddress"),
            Action::Restore(_) => f.write_str("Restore(<protected>)"),
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
            Action::BuildSweep(_) => f.write_str("BuildSweep"),
            Action::BuildTx(_) => f.write_str("BuildTx"),
            Action::Relay(_) => f.write_str("Relay"),
            Action::WalletCreate { name, language, .. } => f
                .debug_struct("WalletCreate")
                .field("name", name)
                .field("password", &"<redacted>")
                .field("language", language)
                .finish(),
            Action::WalletDaemonStart => f.write_str("WalletDaemonStart"),
            Action::WalletDaemonStop => f.write_str("WalletDaemonStop"),
            Action::NodeListFetch { refresh } => f
                .debug_struct("NodeListFetch")
                .field("refresh", refresh)
                .finish(),
            Action::NodeProbe { targets } => f
                .debug_struct("NodeProbe")
                .field("targets", &targets.len())
                .finish(),
            Action::NodeUseRemote { address, trusted } => f
                .debug_struct("NodeUseRemote")
                .field("address", address)
                .field("trusted", trusted)
                .finish(),
            Action::NodeLocal => f.write_str("NodeLocal"),
        }
    }
}

impl Action {
    /// The label the status line shows while this runs.
    pub fn label(&self) -> String {
        match self {
            Action::Native(job) => job.label().into(),
            Action::Annotation(_) => "Reading or updating wallet annotation".into(),
            Action::PrepareTool(_) => "Reading wallet identity".into(),
            Action::Advanced(_) => "Running wallet tool".into(),
            Action::Accounts(_) => "Updating accounts and contacts".into(),
            Action::NewAddressForm { .. } => "Reading wallet identity".into(),
            Action::NewAddress { .. } => "Creating receiving address".into(),
            Action::Restore(_) => "Restoring wallet".into(),
            Action::NodeUp => "Starting your node".into(),
            Action::NodeDown => "Stopping your node".into(),
            Action::WalletOpen { name, .. } => format!("Opening {name}"),
            Action::WalletRefresh { .. } => "Checking for payments".into(),
            Action::BuildSweep(_) => "Preparing send-all fee".into(),
            Action::BuildTx(_) => "Checking the fee".into(),
            Action::Relay(_) => "Sending".into(),
            Action::WalletCreate { name, .. } => format!("Creating {name}"),
            Action::WalletDaemonStart => "Starting the wallet service".into(),
            Action::WalletDaemonStop => "Stopping the wallet service".into(),
            Action::NodeListFetch { .. } => "Getting the node list".into(),
            Action::NodeProbe { targets } => format!("Checking {} nodes", targets.len()),
            Action::NodeUseRemote { address, .. } => format!("Switching to {address}"),
            Action::NodeLocal => "Switching to your own node".into(),
        }
    }

    /// The deadline the status line counts against, so `12s / 120s` is honest.
    pub fn deadline_sec(&self, ctx: &Context) -> u64 {
        match self {
            Action::Native(job) => {
                if job.held() {
                    15 * 60
                } else {
                    ctx.timeouts.open.as_secs()
                }
            }
            Action::NodeUp | Action::NodeDown => ctx.timeouts.unit_probe.as_secs(),
            Action::WalletOpen { .. }
            | Action::Restore(_)
            | Action::NewAddress { .. }
            | Action::NewAddressForm { .. }
            | Action::Accounts(_)
            | Action::Annotation(_)
            | Action::PrepareTool(_)
            | Action::Advanced(_) => ctx.timeouts.open.as_secs(),
            Action::WalletRefresh { .. } => ctx.timeouts.refresh.as_secs(),
            Action::BuildTx(_) | Action::BuildSweep(_) => ctx.timeouts.transfer.as_secs(),
            Action::Relay(_) => ctx.timeouts.relay.as_secs(),
            // A create waits on the new wallet's first `set_daemon`/`auto_refresh`, which in
            // remote mode is over the internet — the same deadline `wallet open` uses (§5b.2).
            Action::WalletCreate { .. } => ctx.timeouts.open.as_secs(),
            Action::WalletDaemonStart | Action::WalletDaemonStop => {
                ctx.timeouts.unit_probe.as_secs()
            }
            // Both curl paths bound themselves (§2n.3): a list fetch is two 20 s documents, and
            // a probe sweep is `ceil(n / 8)` rounds of 5 s.
            Action::NodeListFetch { .. } => 45,
            Action::NodeProbe { targets } => {
                7 * (targets.len().div_ceil(nodes::PROBE_CONCURRENCY)).max(1) as u64
            }
            Action::NodeUseRemote { .. } | Action::NodeLocal => {
                ctx.timeouts.systemctl.as_secs() * 3 + ctx.timeouts.refresh.as_secs() * 2
            }
        }
    }

    /// `Action::Relay` **ignores the StopSignal** (§7a): once `relay_tx` is on the wire,
    /// cancelling tells you nothing about whether the node got it, and the `pending.json`
    /// record is the mechanism that covers it.
    pub fn cancellable(&self) -> bool {
        !matches!(self, Action::Relay(_))
            && !matches!(self, Action::Native(job) if job.submitting())
    }

    /// Would this action explain a wallet-RPC poll that does not answer? The status badge asks, so
    /// that `down` is never printed for a daemon fmonero is itself starting, creating a wallet in,
    /// opening, refreshing, spending from or restarting under a mode switch (FINDING D). The
    /// **complement** of the four node-only actions on purpose: a new wallet action is busy by
    /// default, so the badge's honesty does not depend on this list being extended.
    pub fn wallet_busy(&self) -> bool {
        use Action::{NodeDown, NodeListFetch, NodeProbe, NodeUp};
        !matches!(
            self,
            NodeUp | NodeDown | NodeListFetch { .. } | NodeProbe { .. }
        )
    }
}

/// What an action produced, in the form the model needs to go on.
pub(super) enum Outcome {
    AnnotationForm {
        what: super::Prompt,
        title: String,
        field: String,
        value: String,
    },
    Native(super::native::Reply),
    ToolReady(
        super::advanced::Tool,
        fmonero_core::wallet_ops::WalletIdentity,
    ),
    Advanced(super::advanced::ResultView),
    Accounts(super::accounts::Reply),
    NewAddressForm {
        wallet: fmonero_core::wallet_ops::WalletIdentity,
        account: u32,
    },
    AddressCreated(fmonero_core::receive::ReceivingAddress),
    Restored(fmonero_core::restore::Restored),
    /// A note for the status line (node up/down, wallet refresh).
    Note(String),
    /// The wallet that was opened, so the model can reload its screens.
    Opened(String),
    /// A built, unbroadcast plan — the one the confirm modal renders and `Relay` consumes.
    Built(Box<send::Plan>),
    /// The relayed txids, in relay order.
    Relayed(Vec<String>),
    // ---- v5.5 -----------------------------------------------------------------
    /// A successful create opens guided encrypted recovery in the presentation layer.
    Created {
        name: String,
    },
    /// A refreshed list, for the picker to render.
    NodeList(Box<NodeList>),
    /// The probes, in the order the targets were given.
    Probes(Vec<(String, Probe)>),
    /// A node switch, with the wallet that must be reopened when one was open (§2m.5 step 9).
    NodeSwitched {
        address: String,
        must_reopen: Option<String>,
    },
}

/// A failed action, reduced to what the TUI shows and exits with (§4a).
#[derive(Clone, Debug)]
pub struct ActionErr {
    pub code: i32,
    /// §4a's code name. **No longer rendered** (v5.6, §2o rule 5: an internal identifier is
    /// never shown as prose — it belongs in `--json`), and kept because it is the shape core
    /// hands over and what the exit-code test asserts.
    #[allow(dead_code)]
    pub code_name: String,
    pub text: String,
    /// The CLI's runnable command. **Never rendered by the TUI** (v5.6, §2o.1: every hint in
    /// here names an in-app action), and kept for the same reason as `code_name`.
    #[allow(dead_code)]
    pub hint: String,
    /// The in-app next step, which **is** rendered — appended to `text` as its second sentence
    /// (v5.6, §2o.1). `None` for the errors that have no in-app route, and the toast is then the
    /// sentence alone.
    pub action: Option<String>,
    pub interrupted: bool,
}

impl ActionErr {
    /// What the toast says: the situation, then what to do about it (§2o rule 2).
    pub fn toast(&self) -> String {
        match &self.action {
            Some(a) => format!("{} {}", self.text, a),
            None => self.text.clone(),
        }
    }
}

impl From<Error> for ActionErr {
    fn from(e: Error) -> ActionErr {
        ActionErr {
            code: e.exit_code(),
            code_name: e.code_name().to_string(),
            text: e.to_string(),
            hint: e.hint(),
            action: e.action(),
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
            Action::Native(job) => (*job).execute(ctx, cancel).map(Outcome::Native),
            Action::Annotation(job) => job.execute(ctx),
            Action::PrepareTool(tool) => {
                let w = WalletClient::from_ctx(ctx)?;
                fmonero_core::wallet_ops::identity(ctx, &w)
                    .map(|identity| Outcome::ToolReady(tool, identity))
            }
            Action::Advanced(job) => job.execute(ctx, cancel).map(Outcome::Advanced),
            Action::Accounts(job) => job.execute(ctx).map(Outcome::Accounts),
            Action::NewAddressForm { account } => {
                let w = WalletClient::from_ctx(ctx)?;
                fmonero_core::wallet_ops::identity(ctx, &w)
                    .map(|wallet| Outcome::NewAddressForm { wallet, account })
            }
            Action::NewAddress {
                wallet,
                account,
                label,
            } => {
                let w = WalletClient::from_ctx(ctx)?;
                fmonero_core::receive::new_address(ctx, &w, &wallet, account, &label)
                    .map(Outcome::AddressCreated)
            }
            Action::Restore(request) => {
                let ctx = ctx.clone().with_wallet_probe_at_refresh();
                let w = WalletClient::from_ctx(&ctx)?;
                fmonero_core::restore::restore(&ctx, &w, request, cancel).map(Outcome::Restored)
            }
            Action::NodeUp => {
                let s = node_session::up(ctx, None, cancel)?;
                Ok(Outcome::Note(if s.already_running {
                    "Your node was already running.".into()
                } else {
                    format!("Your node started in {}s.", s.waited_sec)
                }))
            }
            Action::NodeDown => {
                node_session::down(ctx, cancel)?;
                Ok(Outcome::Note("Your node is stopped.".into()))
            }
            Action::WalletOpen { name, password } => {
                let ctx = ctx.clone().with_wallet_probe_at_refresh();
                let w = WalletClient::from_ctx(&ctx)?;
                let opened = wallet_session::open_secret(&ctx, &w, &name, password, cancel)?;
                Ok(Outcome::Opened(opened.name))
            }
            Action::WalletRefresh { start_height } => {
                let w = WalletClient::from_ctx(ctx)?;
                // Through `wallet_session::refresh`: the TUI's `R` is one of the three callers
                // of "refresh, then record that we did" (§5b, §5's `wallet.last_refresh_at`).
                let r = fmonero_core::wallet_session::refresh(ctx, &w, start_height, cancel)?;
                Ok(Outcome::Note(format!(
                    "Checked {} blocks.{}",
                    r.blocks_fetched,
                    if r.received_money {
                        " Money came in."
                    } else {
                        ""
                    }
                )))
            }
            Action::BuildTx(req) => {
                let w = WalletClient::from_ctx(ctx)?;
                // No node client in remote mode — `from_ctx` refuses (§2c) — and step 7 is a
                // bounded `refresh` instead (§2m.3).
                let n = match fmonero_core::NodeClient::from_ctx(ctx) {
                    Ok(n) => Some(n),
                    Err(e) if e.reason() == Some("remote_node") => None,
                    Err(e) => return Err(e),
                };
                let gate = match n.as_ref() {
                    Some(n) => send::NodeGate::Local(n),
                    None => send::NodeGate::Remote,
                };
                let plan = send::build(ctx, &w, gate, &req, cancel)?;
                send::check_max_fee(&plan)?;
                Ok(Outcome::Built(Box::new(plan)))
            }
            Action::BuildSweep(req) => {
                let w = WalletClient::from_ctx(ctx)?;
                // No node client in remote mode — `from_ctx` refuses (§2c) — and step 7 is a
                // bounded `refresh` instead (§2m.3).
                let n = match fmonero_core::NodeClient::from_ctx(ctx) {
                    Ok(n) => Some(n),
                    Err(e) if e.reason() == Some("remote_node") => None,
                    Err(e) => return Err(e),
                };
                let gate = match n.as_ref() {
                    Some(n) => send::NodeGate::Local(n),
                    None => send::NodeGate::Remote,
                };
                let plan = send::build_sweep(ctx, &w, gate, &req, cancel)?;
                send::check_max_fee(&plan)?;
                Ok(Outcome::Built(Box::new(plan)))
            }
            Action::Relay(plan) => {
                let w = WalletClient::from_ctx(ctx)?;
                let txids = send::relay(ctx, &w, plan, &say, cancel)?;
                Ok(Outcome::Relayed(txids))
            }
            // ---- v5.5 ---------------------------------------------------------
            Action::WalletCreate {
                name,
                password,
                language,
            } => {
                let ctx = ctx.clone().with_wallet_probe_at_refresh();
                let w = WalletClient::from_ctx(&ctx)?;
                // **The same sequence `wallet create` calls** (§4.1): `ensure_running`, then
                // `create_wallet`, then the last three steps of `wallet_session::open` on the
                // wallet `create_wallet` left open (`wallet_rpc_server.cpp:3639`). The TUI adds
                // no step and skips none, which is why this is one core call and not four.
                let created = wallet_session::create_secret(
                    &ctx, &w, &name, password, &language, &say, cancel,
                )?;
                Ok(Outcome::Created { name: created.name })
            }
            Action::WalletDaemonStart => {
                let w = WalletClient::from_ctx(ctx)?;
                // The SAME function `wallet open` uses (§7a.5), so it probes first, then runs
                // §2i's preflight — which is what turns a missing unit or a missing binary into
                // a millisecond `unit_missing` / `not_installed` with a runnable hint instead of
                // a 30 s wait — and only then starts.
                let started = units::ensure_running(
                    ctx,
                    &ctx.settings.wallet.unit,
                    &|| w.get_version().is_ok(),
                    ctx.timeouts.unit_probe,
                    cancel,
                )?;
                Ok(Outcome::Note(if started {
                    "The wallet service is running.".into()
                } else {
                    "The wallet service was already running.".into()
                }))
            }
            Action::WalletDaemonStop => {
                units::stop(ctx, &ctx.settings.wallet.unit, cancel)?;
                Ok(Outcome::Note("The wallet service is stopped.".into()))
            }
            Action::NodeListFetch { refresh } => {
                let list = nodes::list(
                    ctx,
                    nodes::ListOpts {
                        refresh,
                        ..Default::default()
                    },
                    cancel,
                )?;
                Ok(Outcome::NodeList(Box::new(list)))
            }
            Action::NodeProbe { targets } => {
                let probes = nodes::probe_many(ctx, &targets, cancel);
                let paired: Vec<(String, Probe)> = targets
                    .iter()
                    .map(|t| t.address.clone())
                    .zip(probes)
                    .collect();
                // The configured node's probe is ALSO cached in `state.json`, so `status --json`
                // and the bar widget can render a remote height without spawning (§2n.4, §2e) —
                // and only when the address still matches the one in force (§2j).
                if let Some(configured) = nodes::configured_address(ctx) {
                    if let Some((_, p)) = paired.iter().find(|(a, _)| *a == configured) {
                        let _ = fmonero_core::state::note_remote_probe(ctx, &configured, p);
                    }
                }
                nodes::record_probes(ctx, &paired)?;
                Ok(Outcome::Probes(paired))
            }
            Action::NodeUseRemote { address, trusted } => {
                let change = node_mode::set_remote(ctx, &address, trusted, false, cancel)?;
                Ok(Outcome::NodeSwitched {
                    address: change.address,
                    must_reopen: change.wallet_must_reopen,
                })
            }
            Action::NodeLocal => {
                let change = node_mode::set_local(ctx, false, cancel)?;
                Ok(Outcome::NodeSwitched {
                    address: change.address,
                    must_reopen: change.wallet_must_reopen,
                })
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
    pub cancel: CancelToken,
}

impl Subscription<Msg> for ActionSub {
    fn id(&self) -> SubId {
        self.id
    }

    fn run(&self, tx: Sender<Msg>, stop: StopSignal) {
        let Some(action) = self.slot.lock().ok().and_then(|mut s| s.take()) else {
            return; // an empty slot: the runtime restarted an unchanged subscription
        };
        let cancel = self.cancel.clone();
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
        let _ = tx.send(Msg::ActionFinished(self.id, Box::new(outcome)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debugging_an_open_never_prints_the_password() {
        let a = Action::WalletOpen {
            name: "w".into(),
            password: fmonero_core::SecretString::new("hunter2".into()).unwrap(),
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
        assert_eq!(Action::NodeUp.label(), "Starting your node");
        assert_eq!(
            Action::WalletOpen {
                name: "main".into(),
                password: fmonero_core::SecretString::empty()
            }
            .label(),
            "Opening main"
        );
    }
}
