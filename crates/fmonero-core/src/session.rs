//! The ONE implementation of the `wallet open`/`close` and `node up`/`down`/`restart`
//! sequences (DESIGN.md §5b, §4.1, §4.2, §2i).
//!
//! Signature-only at the Phase 1a gate (§8). Both modules exist because each sequence has
//! **two** independently written callers — the CLI subcommand and the TUI's modal or key —
//! and §2a.3 already settles what happens to an invariant that lives in two places: it
//! holds in one. The two steps most easily lost are named in the doc comments below,
//! because losing either is silent.

use std::time::Duration;

use crate::address::NetType;
use crate::context::Context;
use crate::error::Error;
use crate::proc::CancelToken;
use crate::rpc::WalletClient;

/// `wallet open` / `wallet close` (§4.1, §4.2).
pub mod wallet_session {
    use super::*;

    /// What an open produced.
    #[derive(Clone, Debug)]
    pub struct Opened {
        pub unit_started: bool,
        pub name: String,
        /// The wallet's **own** nettype, read from its primary address — the §2a.6
        /// send-path authority, which cannot drift from the daemon the way
        /// `settings.node.network` can.
        pub net: NetType,
    }

    /// `units::ensure_running`, then `open_wallet`, then **exactly one**
    /// `set_daemon { address, trusted, username, password }` carrying the node credential
    /// from `Login::from_monerod_conf` (both fields omitted when there is none — the §2i
    /// `system_unit` layout), then `auto_refresh { enable: true, period: 20 }` (§2d), then
    /// `state::set_last_open(name, net)`.
    ///
    /// **The order is not free.** `on_set_daemon` answers `-13 NOT_OPEN` with no wallet
    /// loaded (`wallet_rpc_server.cpp:4692`), so issuing it first would fail every time,
    /// never install the credential, and present as "the node is unreachable" — while
    /// without it every wallet→daemon call against fmonero's own authenticated node gets a
    /// 401 and the wallet can never sync (§4.2). A `-38 NO_DAEMON_CONNECTION` from
    /// `set_daemon` does **not** fail the open: the wallet is open, it simply has no daemon
    /// yet. `password` is moved in and zeroed before return.
    pub fn open(
        ctx: &Context,
        w: &WalletClient,
        name: &str,
        password: String,
        cancel: &CancelToken,
    ) -> Result<Opened, Error> {
        // The units are written DISABLED and started on demand (§4.2), and `ensure_running`
        // probes first — so this is a no-op against `fmonero-mock` with no unit installed.
        let unit_started = crate::units::ensure_running(
            ctx,
            &ctx.settings.wallet.unit,
            &|| w.get_version().is_ok(),
            ctx.timeouts.unit_probe,
            cancel,
        )?;
        let opened = open_already_running(ctx, w, name, password);
        // `password` is moved in and zeroed before return (§5b). It is owned, so this is the
        // only copy; `open_already_running` takes it by value and drops it.
        opened.map(|net| Opened {
            unit_started,
            name: name.to_string(),
            net,
        })
    }

    /// What a create produced (v5.5). It carries [`seed_backup_hint`] so the CLI's footer and
    /// §7a.5's seed-backup modal render **one** string from core and not two wordings.
    #[derive(Clone, Debug)]
    pub struct Created {
        pub unit_started: bool,
        pub name: String,
        pub net: NetType,
        pub seed_backup_hint: String,
    }

    /// The one text that tells a user how to back up a seed (§2b.2), so the CLI's footer, the
    /// `wallet create` payload and §7a.5's modal cannot drift.
    ///
    /// **The close line is v5.5, revision 11, and it is what makes the instruction correct.**
    /// `on_create_wallet` leaves the new wallet **open** in `monero-wallet-rpc`
    /// (`wallet_rpc_server.cpp:3639`) and `create` below then runs the last three steps of
    /// [`open`] on it — so this text, delivered the moment a create finishes, was telling the
    /// user to open the same wallet file in a **second** wallet2 process while the daemon holds
    /// it open and will write its cache later. Stakes are low for a fresh empty wallet (the seed
    /// lives in the `.keys` file either way), but the same string is printed for wallets that
    /// will be funded, and "run this now" with no close step is an instruction this codebase
    /// would not write if the concurrency were stated out loud.
    pub fn seed_backup_hint(ctx: &Context, name: &str) -> String {
        format!(
            "Back up your seed before you put money in this wallet. fmonero never shows \
             it.\nWith the wallet closed, in a terminal:\n  \
             monero-wallet-cli --wallet-file {}/{name} --offline\n  then type: seed",
            ctx.settings.wallet.dir.display()
        )
    }

    /// `units::ensure_running`, `create_wallet`, then **the last three steps of [`open`]** on the
    /// wallet `create_wallet` left open (§4.1). v5.5 moved this sequence into core because the
    /// TUI now creates wallets too (§7a.5), and §2a.3's rule is that a sequence with two callers
    /// has one implementation — the alternative is two places that can each forget the
    /// `set_daemon` step whose absence means the wallet can never sync (§4.2).
    ///
    /// `announce` exists for one reason, and it is v5.3's: **from the moment `create_wallet`
    /// returns the wallet file exists, and every later step can still fail.** The first
    /// `wallet create` against a real public node exited 5 on `set_daemon`, because wallet-rpc
    /// held its daemon mutex across the new wallet's first refresh — so emitting "back up your
    /// seed before funding this wallet" only on the success path would make the one output that
    /// protects money conditional on a step that has nothing to do with it. Core is silent
    /// (§2g), so the sink belongs to the caller: the CLI's is stderr, the TUI's is the Log pane.
    ///
    /// Every **local** refusal — a `/` in the name, an existing wallet, an empty password — is
    /// the caller's, before this is called: they differ per surface (the CLI has
    /// `--allow-empty-password`, the TUI deliberately has no equivalent, §7a.5).
    pub fn create(
        ctx: &Context,
        w: &WalletClient,
        name: &str,
        password: String,
        language: &str,
        announce: &dyn Fn(&str),
        cancel: &CancelToken,
    ) -> Result<Created, Error> {
        crate::settings::validate_seed_language(language)?;
        let unit_started = crate::units::ensure_running(
            ctx,
            &ctx.settings.wallet.unit,
            &|| w.get_version().is_ok(),
            ctx.timeouts.unit_probe,
            cancel,
        )?;
        w.create_wallet(name, &password, language)?;
        let hint = seed_backup_hint(ctx, name);
        announce(&hint);
        // `on_create_wallet` left the wallet OPEN, so the last three steps still have to run:
        // without them the wallet has no daemon credential and no recorded name, `status` would
        // report `wallet.open: true` with `wallet.name: null`, and every wallet→daemon call
        // would 401 (§4.1).
        let net = open_already_running(ctx, w, name, password)?;
        Ok(Created {
            unit_started,
            name: name.to_string(),
            net,
            seed_backup_hint: hint,
        })
    }

    /// `open_wallet`, then the three steps that must follow it.
    ///
    /// `wallet create` reaches the same three through [`open`] — `on_create_wallet` ends
    /// `m_wallet = wal.release()` (`wallet_rpc_server.cpp:3639`), so the new wallet is left
    /// open, and re-opening it with its own password is what lets one code path install the
    /// daemon credential and record the name for both commands (§4.1).
    fn open_already_running(
        ctx: &Context,
        w: &WalletClient,
        name: &str,
        mut password: String,
    ) -> Result<NetType, Error> {
        let result = w.open_wallet(name, &password);
        // Overwrite the buffer before it is freed (§2b.4's rule, applied to the CLI path too).
        // SAFETY-adjacent: `String` may reallocate on push, so write over the bytes in place.
        unsafe {
            for b in password.as_mut_vec().iter_mut() {
                *b = 0;
            }
        }
        drop(password);
        result?;
        install_daemon_and_record(ctx, w, name)
    }

    /// `set_daemon` carrying the node credential, then `auto_refresh`, then
    /// `state::set_last_open`. **In that order**: `on_set_daemon` answers `-13 NOT_OPEN` with
    /// no wallet loaded (`wallet_rpc_server.cpp:4692`), so it can only come after the open.
    fn install_daemon_and_record(
        ctx: &Context,
        w: &WalletClient,
        name: &str,
    ) -> Result<NetType, Error> {
        // The four arguments come from `units::daemon_target` — the same accessor
        // `units::render_wallet_unit` reads, so the unit and the call cannot disagree (§2m.3,
        // §5b). In **remote** mode the credential is `None`, always: fmonero's `rpc-login`
        // belongs to fmonero's own node and means nothing to a stranger's.
        let target = crate::units::daemon_target(ctx);
        let login = if ctx.settings.node.mode.is_remote() {
            None
        } else {
            crate::rpc::Login::from_monerod_conf(&ctx.paths.monerod_conf())?
        };
        if let Err(e) = w.set_daemon(&target.address, target.trusted, login.as_ref(), target.ssl) {
            // §4.2 exempts exactly TWO failures, and the second one **only in local mode**.
            //
            // * a transport failure (`reason: unreachable`) — nothing answered the call; the
            //   wallet is open either way and `status` reports the truth;
            // * `-38` (`reason: remote_address_rejected`) — the address did not **parse**.
            //   `set_daemon` opens no socket: it is `m_http_client->set_server(...)`
            //   (`wallet2.cpp:1409`), a URL parse, and `-38 "Unable to set daemon"`
            //   (`wallet_rpc_server.cpp:4758-4763`) is what a failed parse answers (§1b.9
            //   item 1). v5 attributed it to "wallet2 cannot reach the address", which was
            //   wrong. **In local mode it stays a warning** — `node.url` is loopback-validated
            //   so the case is nearly unreachable, the wallet really is open, and `status`
            //   reports the truth. **In remote mode it is not swallowed** (§2m.3): the address
            //   passed §2m.2's grammar minutes ago, so fmonero's grammar and epee's parser
            //   disagree, and a `wallet open` reported successful for a wallet whose daemon was
            //   rejected is the silence §2j forbids. Exit 4 `backend`,
            //   `reason: remote_address_rejected`, message verbatim, with the `node remote`
            //   hint `WalletClient::set_daemon` already attached.
            //
            // Exit 4 is a whole class, not those two codes: a 401 (`auth_refused`), `-7
            // DENIED`, `-48 DISABLED` and `-23 NO_WALLET_DIR` all land there, and swallowing
            // them would report a successful `wallet open` for a wallet left with NO daemon
            // credential — the step §4.2 calls the one "whose omission means the wallet can
            // never sync".
            let rejected = e.reason() == Some("remote_address_rejected");
            if rejected && ctx.settings.node.mode.is_remote() {
                return Err(e);
            }
            if !rejected && e.reason() != Some("unreachable") {
                return Err(e);
            }
            let _ = crate::state::set_last_error(ctx, &e, None);
        }
        // Explicit rather than inherited, and deliberately NOT raised: a larger period means
        // longer blocking bursts, not fewer stalls (§2d).
        w.auto_refresh(true, AUTO_REFRESH_PERIOD_SEC)?;
        // The wallet's OWN nettype, from its own primary address — the §2a.6 send-path
        // authority, which cannot drift from the daemon the way `settings.node.network` can.
        let net = crate::address::parse(&w.get_address(ctx.settings.wallet.account)?.primary)?.net;
        crate::state::set_last_open(ctx, name, net)?;
        Ok(net)
    }

    /// `DEFAULT_AUTO_REFRESH_PERIOD` (`wallet_rpc_server.cpp:63`), sent explicitly (§2d).
    pub const AUTO_REFRESH_PERIOD_SEC: u32 = 20;

    /// `close_wallet { autosave_current: true }` then `state::clear_last_open`.
    pub fn close(ctx: &Context, w: &WalletClient) -> Result<(), Error> {
        w.close_wallet()?;
        crate::state::clear_last_open(ctx)
    }

    /// `WalletClient::refresh` followed by `state::note_refresh` — the one place the "last
    /// refresh **fmonero** issued" fact is written (v5.2, §5's `wallet.last_refresh_at`).
    ///
    /// Three independently written callers — the CLI's `wallet refresh`, `send`'s §2m.3 remote
    /// gate and the TUI's `R` — and §2a.3 already settles what happens to a step that lives in
    /// three. The qualifier is the whole point: `monero-wallet-rpc`'s own 20 s auto-refresh
    /// (§2d) is invisible to fmonero, so a field named "last refresh" that counted it would be a
    /// claim about the daemon's behaviour we cannot make (§2j).
    ///
    /// A failure to record it is **not** a failure of the refresh: the blocks were fetched.
    pub fn refresh(
        ctx: &Context,
        w: &WalletClient,
        start_height: u64,
        cancel: &CancelToken,
    ) -> Result<crate::rpc::wallet::Refreshed, Error> {
        let r = w.refresh(start_height, cancel)?;
        let _ = crate::state::note_refresh(ctx);
        Ok(r)
    }
}

/// `node up` / `node down` / `node restart` (§4.1, §2i).
pub mod node_session {
    use super::*;

    /// What a start produced.
    #[derive(Clone, Debug)]
    pub struct Started {
        /// `node up` against our own live node: exit 0, no second start (§2i).
        pub already_running: bool,
        pub waited_sec: u64,
        pub reachable: bool,
        pub synchronized: Option<bool>,
    }

    /// `units::detect_layout` (`UserUnit`, else `Error::Safety` with §2i's
    /// `sudo systemctl …` hint) → `units::data_dir_in_use`: no holder ⇒ start; holder equals
    /// `units::main_pid(unit)` ⇒ `already_running: true` with **no second start**; any other
    /// pid ⇒ `Error::Safety{reason:"data_dir_in_use"}` naming it → `units::start` → poll
    /// `get_info` to `timeouts.unit_probe` (`Error::Timeout`, exit 8) → when `wait` is
    /// `Some`, poll `get_info.synchronized` to that duration (`Error::Timeout`, exit 8).
    ///
    /// The single-writer precondition is the only guard against two `monerod` on one LMDB
    /// (§1.2), and it lives here rather than in `units::start` precisely so the TUI's `u`
    /// key cannot skip it.
    pub fn up(
        ctx: &Context,
        wait: Option<Duration>,
        cancel: &CancelToken,
    ) -> Result<Started, Error> {
        let layout = crate::units::detect_layout(ctx)?;
        require_user_unit(ctx, layout, Some("up"))?;
        let unit = ctx.settings.node.unit.clone();
        // Before any systemctl call (§2i, v5.1): the unit file, then `monerod` on PATH. Without
        // the second, this command waits 30 s for a unit that exits 203/EXEC and is restarted
        // every 5 s, and reports exit 8 "nothing is broken" — where §4a says exit 4
        // `not_installed`. It is the state `scripts/install.sh --yes` leaves on a machine
        // Monero has not reached, so it is the first thing an owner meets.
        crate::units::preflight(ctx, &unit, "monerod")?;

        // The §2i single-writer precondition — the only guard against two `monerod` on one
        // LMDB (§1.2). It is a precondition of `up`, never a term in `detect_layout`: it
        // cannot tell our own monerod from a foreign one.
        let mut already_running = false;
        if let Some(pid) = crate::units::data_dir_in_use(ctx, &ctx.settings.node.data_dir)? {
            match crate::units::main_pid(ctx, &unit)? {
                Some(ours) if ours == pid => already_running = true,
                _ => {
                    // **X2: rules 6 and 7 in the sentence and again in the hint's comment.** The
                    // process id is what an operator needs and it moves to `detail`.
                    return Err(Error::safety(format!(
                        "Another program is already using {}. Two nodes on one chain folder \
                         would corrupt it.",
                        ctx.settings.node.data_dir.display()
                    ))
                    .with_reason("data_dir_in_use")
                    .with_detail(format!("chain folder held by pid {pid}"))
                    .with_hint("fmonero status --json"));
                }
            }
        }
        let started_at = std::time::Instant::now();
        if !already_running {
            crate::units::start(ctx, &unit, cancel)?;
        }
        // Return as soon as one `get_info` answers, waiting at most `timeouts.unit_probe`.
        let n = crate::rpc::NodeClient::from_ctx(ctx)?;
        let info;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Interrupted);
            }
            if let Ok(i) = n.get_info() {
                info = i;
                break;
            }
            if started_at.elapsed() >= ctx.timeouts.unit_probe {
                // We started it and it never answered: stop it again rather than leave a
                // `Restart=on-failure` loop behind a command that has already failed (v5.1).
                let stopped = if already_running {
                    String::new()
                } else {
                    crate::units::stop_after_failed_start(ctx, &unit, cancel)
                };
                // **X2: a unit name and an RPC method, in the message a person gets when
                // `node up` times out.** §2o.2 names both explicitly; the unit and the method go
                // to `detail`.
                return Err(Error::timeout(format!(
                    "Your node did not answer within {}s{stopped}",
                    ctx.timeouts.unit_probe.as_secs()
                ))
                .with_detail(format!("{unit}: no get_info answer"))
                .with_hint("fmonero status --json")
                .with_action("The Node screen shows whether it is still starting up."));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let mut info = info;
        if let Some(bound) = wait {
            // `--wait` polls until `synchronized`, bounded by `--wait-timeout`. There is no
            // unbounded default: a command that can block for days is one an agent cannot
            // call (§2d).
            let waiting_since = std::time::Instant::now();
            while !info.synchronized {
                if cancel.is_cancelled() {
                    return Err(Error::Interrupted);
                }
                // `--wait-timeout 0` is the explicit opt-in to "poll until interrupted", and
                // `main.rs` has already refused it without a TTY (§4.1).
                if !bound.is_zero() && waiting_since.elapsed() >= bound {
                    let pct = sync_percent(&info);
                    return Err(Error::timeout(format!(
                        "still syncing at {pct}% after {}s",
                        waiting_since.elapsed().as_secs()
                    ))
                    .with_hint(
                        "fmonero node up --wait --wait-timeout 86400   # or: fmonero status --json",
                    ));
                }
                std::thread::sleep(Duration::from_secs(1));
                info = n.get_info()?;
            }
        }
        Ok(Started {
            already_running,
            waited_sec: started_at.elapsed().as_secs(),
            reachable: true,
            synchronized: Some(info.synchronized),
        })
    }

    /// `NodeClient::mutators(ctx)?.stop_daemon()` if reachable — so LMDB closes cleanly —
    /// then `units::stop`. Layout `UserUnit` only; the mutators type **is** the gate (§2i).
    pub fn down(ctx: &Context, cancel: &CancelToken) -> Result<(), Error> {
        // `down` and `restart` require only the layout; they never consult `data_dir_in_use`,
        // because the holder of our own data dir is our own node (§2i).
        require_user_unit(ctx, crate::units::detect_layout(ctx)?, Some("down"))?;
        let n = crate::rpc::NodeClient::from_ctx(ctx)?;
        // The RPC first, so LMDB closes cleanly; a node that is already down simply does not
        // answer, which is not a failure of `node down`.
        if let Ok(m) = n.mutators(ctx) {
            let _ = m.stop_daemon();
        }
        crate::units::stop(ctx, &ctx.settings.node.unit, cancel)
    }

    /// `down` then `up`.
    ///
    /// The §2i preflight runs **here**, before `down`'s own `systemctl --user stop`: `up` would
    /// refuse a moment later anyway, and a restart that stops the node and then reports
    /// "monerod is not installed" has left the machine worse than it found it (v5.1).
    pub fn restart(ctx: &Context, cancel: &CancelToken) -> Result<Started, Error> {
        require_user_unit(ctx, crate::units::detect_layout(ctx)?, Some("restart"))?;
        crate::units::preflight(ctx, &ctx.settings.node.unit, "monerod")?;
        down(ctx, cancel)?;
        up(ctx, None, cancel)
    }

    fn require_user_unit(
        ctx: &Context,
        layout: crate::units::Layout,
        verb: Option<&str>,
    ) -> Result<(), Error> {
        match layout {
            crate::units::Layout::UserUnit => Ok(()),
            crate::units::Layout::SystemUnit => Err(Error::safety(crate::FOREIGN_NODE_READ_ONLY)
                .with_reason("foreign_node")
                .with_hint("sudo systemctl start monerod   # fmonero does not use sudo")),
            // v5.2, §2m.3: exit **4**, not 2. The arguments are correct; the backend is
            // somewhere else, and an agent's reflex on exit 2 (re-read `--help`) leads nowhere.
            // The message is built where every other remote refusal is (§2a.3).
            crate::units::Layout::Remote => Err(crate::units::remote_node_error(ctx, verb)),
            crate::units::Layout::None => Err(Error::backend(
                "fmonero has not set up the node service yet.",
            )
            .with_reason("unit_missing")
            // `--yes`, because the hint is run, not read: `fmonero setup --units` alone exits
            // 3 `needs_yes`, which is a second round trip for the agent this hint is for.
            .with_hint("fmonero setup --units --yes")),
        }
    }

    /// The §5 formula, which has to special-case `target_height == 0` — it is **0 when
    /// synced**, not equal to `height` (§1b.3).
    fn sync_percent(info: &crate::rpc::node::GetInfo) -> u64 {
        if info.synchronized || info.target_height == 0 {
            return 100;
        }
        info.height * 100 / info.target_height.max(1)
    }
}

/// The two convenience commands of §2m.5, in core because they write settings, render units and
/// call `systemctl` — three core responsibilities — and because `main.rs` is frozen at the 1a
/// gate (§5b, §2k).
///
/// The TUI does **not** call them: it toasts the command (§7a). A modal that restarts a daemon
/// and closes the wallet the session is holding is a screen this design would have to specify a
/// recovery for.
pub mod node_mode {
    use super::*;
    use crate::settings::{NodeMode, Settings, SslSupport};
    use crate::units::{self, FileAction, FileStatus};

    /// What a mode switch did. **IS** §5c's `node remote` / `node local` payload, verbatim —
    /// the two commands share one key set, as `wallet send` and `wallet sweep` do, so an agent
    /// parses one shape and reads `mode` to learn which way it went.
    #[derive(Clone, Debug, serde::Serialize)]
    pub struct ModeChange {
        pub mode: NodeMode,
        /// The address **in force after the change**: `settings.node.url`'s host:port in local
        /// mode, `settings.node.remote` in remote mode (§5c) — both through
        /// `units::daemon_target`.
        pub address: String,
        pub trusted: bool,
        pub ssl_support: SslSupport,
        /// The same `units::FileAction[]` shape and the same renderer `setup` uses (§5b).
        pub actions: Vec<FileAction>,
        pub wallet_unit_restarted: bool,
        /// The wallet that was open and was closed by the restart, and must be reopened
        /// (§2m.5 step 9). Reporting success while `status` still claimed a wallet was open
        /// would be a stale fact about money.
        pub wallet_must_reopen: Option<String>,
        /// `false` on an idempotent second run.
        pub changed: bool,
        pub dry_run: bool,
    }

    /// §2m.5's steps, for `node remote ADDR [--trusted]`.
    ///
    /// `addr` is validated by [`Settings::parse_remote_address`] **first**, so an unparseable
    /// one costs nothing and touches nothing (§2m.2).
    pub fn set_remote(
        ctx: &Context,
        addr: &str,
        trusted: bool,
        dry_run: bool,
        cancel: &CancelToken,
    ) -> Result<ModeChange, Error> {
        let address = Settings::parse_remote_address(addr)?;
        apply(
            ctx,
            NodeMode::Remote,
            Some((address, trusted)),
            dry_run,
            cancel,
        )
    }

    /// The same steps with `node.mode = local` and no ADDR.
    ///
    /// `node.remote` and `node.remote_trusted` are **left where they are** (§2m.1): they are
    /// inert in local mode, and discarding an address the user will want back is a discourtesy
    /// with no benefit.
    pub fn set_local(
        ctx: &Context,
        dry_run: bool,
        cancel: &CancelToken,
    ) -> Result<ModeChange, Error> {
        apply(ctx, NodeMode::Local, None, dry_run, cancel)
    }

    /// The steps both commands share (§2m.5). Neither starts a daemon and neither runs
    /// `node up`: §2i's single-writer precondition is the only thing allowed to start
    /// `monerod`, and folding a start into a mode switch would route around it.
    fn apply(
        ctx: &Context,
        mode: NodeMode,
        remote: Option<(String, bool)>,
        dry_run: bool,
        cancel: &CancelToken,
    ) -> Result<ModeChange, Error> {
        // The settings as they would be. Nothing is saved until step 6, and the render below
        // reads THESE, so `--dry-run` shows the unit the switch would write.
        let mut next = ctx.settings.clone();
        if let Some((address, trusted)) = remote {
            next.node.remote = address;
            next.node.remote_trusted = trusted;
        }
        next.node.mode = mode;
        let settings_changed = next != ctx.settings;
        let mut next_ctx = ctx.clone();
        next_ctx.settings = next.clone();
        let target = units::daemon_target(&next_ctx);
        let done = |actions: Vec<FileAction>,
                    changed: bool,
                    wallet_unit_restarted: bool,
                    wallet_must_reopen: Option<String>| ModeChange {
            mode,
            address: target.address.clone(),
            trusted: target.trusted,
            ssl_support: target.ssl,
            actions,
            wallet_unit_restarted,
            wallet_must_reopen,
            changed,
            dry_run,
        };

        // Step 2: `--dry-run` prints the plan and the unit diff and exits 0, **without needing
        // `--yes`** and **before the two money guards** — it writes nothing, restarts nothing
        // and can abort no relay, so blocking a preview on a send in flight would refuse the
        // safe half of the command for a risk it does not carry (§2m.5).
        if dry_run {
            let actions = units::install(&next_ctx, true)?;
            let changed =
                settings_changed || actions.iter().any(|a| a.status == FileStatus::Planned);
            return Ok(done(actions, changed, false, None));
        }

        // Steps 5 and 6, the money interlock (§2m.5). This is the one command whose purpose is
        // to change the node a send is **resolved** against: after the switch §2a.4's resolve
        // step would ask a different node for a transaction it need not hold, get "not found"
        // for one that really was broadcast, and block every later send until the user typed
        // `clear-pending` — the habit §2a.4 exists to prevent. The lock is held to the end of
        // the call, so the save, the render and the restart are one critical section.
        let _lock = crate::send::take_send_lock(ctx)?;
        crate::send::refuse_if_send_in_flight(ctx)?;

        // Read **before** the render: a running `monero-wallet-rpc` holds `--daemon-address`
        // from its argv, so whether it must be restarted is a fact about the old state.
        let was_active = units::state(ctx, &ctx.settings.wallet.unit)?.active;
        // Step 7: the three keys in ONE save, so a crash cannot leave `mode == remote` with no
        // address — §2m.1's invariant is a property of the file, not of three writes.
        if settings_changed {
            next.save(ctx)?;
        }
        // Step 8: both units and `monerod.conf` are re-rendered in **both** modes. The node
        // unit and the conf are unused in remote mode, but `node local --yes` must find them
        // and the doctor's `units_present` compares them byte for byte in every mode.
        let actions = units::install(&next_ctx, false)?;
        let wrote = actions.iter().any(|a| a.status == FileStatus::Written);
        // **The WALLET unit specifically** (v5.5, revision 4). The reason a restart is needed at
        // all is that a running `monero-wallet-rpc` holds `--daemon-address` in its argv, so the
        // only file whose change can justify one is the file that carries that argv — not
        // `monerod.conf` and not the node unit, both of which are re-rendered in both modes and
        // neither of which a running wallet daemon has read. Keyed on `wrote` alone, a switch
        // that changed only the node unit would stop and start the wallet daemon and (step 10)
        // close an open wallet for a change it could not see.
        let wallet_unit_changed = actions.iter().any(|a| {
            a.status == FileStatus::Written
                && a.path.file_name().and_then(|n| n.to_str())
                    == Some(ctx.settings.wallet.unit.as_str())
        });

        // Step 9: restart the wallet unit **iff it was active and its own rendered argv changed**.
        // Keying the restart on that rather than on "the mode changed" is what makes a second run
        // idempotent — and it is exactly right: if the rendered argv is identical there is nothing
        // for a restart to install.
        let mut wallet_unit_restarted = false;
        let mut wallet_must_reopen = None;
        if was_active && wallet_unit_changed {
            let open_now = crate::state::last_open(ctx)?.map(|(name, _)| name);
            units::stop(ctx, &ctx.settings.wallet.unit, cancel)?;
            units::start(ctx, &ctx.settings.wallet.unit, cancel)?;
            wallet_unit_restarted = true;
            // Step 10: the restart closed whatever was open, so say so rather than leave
            // `status` claiming a wallet that is gone.
            if let Some(name) = open_now {
                crate::state::clear_last_open(ctx)?;
                wallet_must_reopen = Some(name);
            }
        }
        Ok(done(
            actions,
            settings_changed || wrote,
            wallet_unit_restarted,
            wallet_must_reopen,
        ))
    }
}
