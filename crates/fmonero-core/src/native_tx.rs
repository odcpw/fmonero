//! Held Monero CLI reviews for offline signing and signed submission.
//! No yes is written until exact review is returned and explicitly confirmed.
use crate::{
    artifacts::{self, ArtifactReceipt, PrivateDirectory},
    native_process::{self, NativeProcess},
    wallet_ops::{self, WalletIdentity},
    CancelToken, Context, Error, Piconero, SecretString, WalletClient,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub artifact_sha256: String,
    pub transactions: u32,
    pub input_amount: Piconero,
    pub fee: Piconero,
    pub recipients: Vec<crate::cold::Recipient>,
    pub change_amount: Piconero,
    pub change_address: Option<String>,
    pub minimum_ring_size: u32,
    pub engine_review: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct NativeReturn {
    pub wallet_reopened: bool,
    pub warning: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SignedArtifact {
    pub artifact: ArtifactReceipt,
    pub txids: Vec<String>,
    pub returned: NativeReturn,
}
#[derive(Clone, Debug, Serialize)]
pub struct Submitted {
    pub txids: Vec<String>,
    pub unresolved: u32,
    pub all_submitted: bool,
    pub returned: NativeReturn,
    pub warning: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSubmission {
    pub wallet: WalletIdentity,
    pub artifact_sha256: String,
    pub transactions: u32,
    pub unknown_txids: u32,
    pub proven_txids: Vec<String>,
    pub attempted_at: i64,
}
struct Held {
    ctx: Context,
    wallet: WalletIdentity,
    password: SecretString,
    process: Option<NativeProcess>,
    stage: PrivateDirectory,
    review: Review,
    _lock: File,
}
pub struct HeldUnsigned(Held);
pub struct HeldSigned(Held);

/// Connects only through Monero, under the exact configured TLS/proxy policy.
/// Signed submission needs a reachable daemon even to reach the review prompt.
pub fn review_signed(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    password: SecretString,
    path: &Path,
    cancel: &CancelToken,
) -> Result<HeldSigned, Error> {
    begin(ctx, w, wallet, password, path, cancel, true).map(HeldSigned)
}
/// Uses --offline: Monero's native unsigned review and signing have no node access.
pub fn review_unsigned(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    password: SecretString,
    path: &Path,
    cancel: &CancelToken,
) -> Result<HeldUnsigned, Error> {
    begin(ctx, w, wallet, password, path, cancel, false).map(HeldUnsigned)
}
fn begin(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    password: SecretString,
    path: &Path,
    cancel: &CancelToken,
    signed: bool,
) -> Result<Held, Error> {
    if password.expose_secret().contains(['\r', '\n']) {
        return Err(Error::usage(
            "This native wallet workflow requires a password without line breaks.",
        ));
    }
    let bytes = artifacts::read(ctx, path)?;
    let magic = if signed {
        b"Monero signed tx set".as_slice()
    } else {
        b"Monero unsigned tx set".as_slice()
    };
    if !bytes.starts_with(magic) {
        return Err(Error::usage(
            "Choose the native Monero transaction artifact for this operation.",
        ));
    }
    let digest = artifacts::hex(&Sha256::digest(&bytes));
    let lock = wallet_ops::guard(ctx, w, wallet)?;
    crate::wallet_dir::validate_name(&wallet.name)?;
    let connection = if signed {
        Some(crate::transport::DaemonConnection::from_context(ctx)?)
    } else {
        None
    };
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let stage = PrivateDirectory::new(&ctx.paths.run_dir)?;
    stage.write(
        if signed {
            "signed_monero_tx"
        } else {
            "unsigned_monero_tx"
        },
        &bytes,
    )?;
    drop(bytes);
    let memory = native_process::sealed_input(password.expose_secret().as_bytes())?;
    let mut args = native_process::base_args(wallet.network);
    args.extend([
        "--wallet-file".into(),
        ctx.settings
            .wallet
            .dir
            .join(&wallet.name)
            .to_string_lossy()
            .into_owned(),
        "--password-file".into(),
        "/proc/self/fd/3".into(),
        "--shared-ringdb-dir".into(),
        "ringdb".into(),
    ]);
    if let Some(connection) = connection {
        args.extend(connection.daemon_args());
    } else {
        args.push("--offline".into());
    }
    // Local daemon authentication is a sealed config fd, never argv/environment.
    let daemon_config = if signed && !ctx.settings.node.mode.is_remote() {
        crate::rpc::Login::from_monerod_conf(&ctx.paths.monerod_conf())?
            .map(|login| protected_daemon_config(&login))
            .transpose()?
    } else {
        None
    };
    if daemon_config.is_some() {
        args.extend(["--config-file".into(), "/proc/self/fd/4".into()]);
    }
    args.extend([
        "--command".into(),
        if signed {
            "submit_transfer".into()
        } else {
            "sign_transfer".into()
        },
    ]);
    w.close_wallet()?;
    crate::state::clear_last_open(ctx)?;
    let startup = (|| -> Result<(NativeProcess, Review), Error> {
        let mut inputs = vec![&memory];
        if let Some(config) = &daemon_config {
            inputs.push(config);
        }
        let mut process = NativeProcess::start_review(
            &args,
            &stage.path(),
            &inputs,
            Duration::from_secs(15 * 60),
        )?;
        let until = std::time::Instant::now() + ctx.timeouts.open;
        let mut unlock_sent = false;
        loop {
            process.pump(cancel)?;
            if std::time::Instant::now() >= until {
                return Err(Error::upstream(
                    "Monero did not reach transaction review in time. Nothing was approved.",
                ));
            }
            let text = std::str::from_utf8(process.output.as_slice()).map_err(|_| {
                Error::backend("Monero emitted invalid review text. Nothing was approved.")
            })?;
            if let Some(start) = text.rfind("Loaded ") {
                // input_line writes no newline; include its complete flushed
                // yes/no suffix before returning a held review. Otherwise a
                // split read could leave prompt bytes in the receipt window.
                if let Some(end) = text[start..].find("Is this okay?  (Y/Yes/N/No): ") {
                    let prompt = &text[start..start + end + "Is this okay?".len()];
                    let mut review = parse_review(prompt, wallet.network)?;
                    review.artifact_sha256 = digest.clone();
                    return Ok((process, review));
                }
            }
            // sign_transfer may explicitly unlock keys after initial wallet load.
            // One exact password prompt is supported; no generic prompt response.
            process.unlock_if_prompted(&password, &mut unlock_sent, cancel)?;
            if process.exited() {
                return Err(Error::backend("Monero could not review this artifact. Check the wallet, password, artifact and daemon; nothing was approved."));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    match startup {
        Ok((process, review)) => Ok(Held {
            ctx: ctx.clone(),
            wallet: wallet.clone(),
            password,
            process: Some(process),
            stage,
            review,
            _lock: lock,
        }),
        Err(error) => {
            // startup's child has been dropped/reaped before this reopen attempt.
            let returned = reopen(ctx, w, wallet, &password);
            Err(error.with_action(if returned.wallet_reopened {
                "The original wallet is open. Review the input before trying again."
            } else {
                "Open the original wallet again to continue."
            }))
        }
    }
}
fn protected_daemon_config(login: &crate::rpc::Login) -> Result<File, Error> {
    // Boost's config grammar strips comments/outer whitespace. Fail before the
    // wallet is closed for values that cannot be represented literally and safely.
    if login.user.is_empty()
        || login.pass.is_empty()
        || login.user.contains(':')
        || login
            .user
            .chars()
            .chain(login.pass.chars())
            .any(|c| c.is_whitespace() || c.is_control() || c == '#')
    {
        return Err(Error::safety("The local daemon credential cannot be represented safely in Monero's protected configuration input."));
    }
    use std::io::Write;
    let mut buffer = crate::secrets::PrivateBuffer::new(2 * crate::secrets::MAX_SECRET_BYTES);
    writeln!(&mut buffer, "daemon-login={}:{}", login.user, login.pass).map_err(|_| {
        Error::safety("The local daemon credential exceeds its protected input bound.")
    })?;
    native_process::sealed_input(buffer.as_slice())
}
fn reopen(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    password: &SecretString,
) -> NativeReturn {
    let attempt = (|| -> Result<(), Error> {
        w.open_wallet(&wallet.name, password.expose_secret())?;
        let address = w.get_address(0)?.primary;
        if address != wallet.primary_address
            || crate::address::parse(&address)?.net != wallet.network
        {
            let _ = w.close_wallet();
            let _ = crate::state::clear_last_open(ctx);
            return Err(wallet_ops::stale());
        }
        crate::wallet_session::install_daemon_and_record(ctx, w, &wallet.name)?;
        Ok(())
    })();
    match attempt {
        Ok(()) => NativeReturn {
            wallet_reopened: true,
            warning: None,
        },
        Err(e) => NativeReturn {
            wallet_reopened: false,
            warning: Some(format!(
                "Reopening the recorded wallet was not confirmed: {e}"
            )),
        },
    }
}
/// Explicit recovery of observation access only. The reviewed uncertainty record
/// stays intact and continues to block all ordinary payment/mutation guards.
/// This is a native-specific recovery path: it never accepts an alternative
/// identity and does not relax the shared normal wallet/session guards.
pub fn reopen_pending_for_inspection(
    ctx: &Context,
    w: &WalletClient,
    expected: &PendingSubmission,
    password: SecretString,
    cancel: &CancelToken,
) -> Result<NativeReturn, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    if pending(ctx)?.as_ref() != Some(expected) {
        return Err(Error::safety(
            "The native submission record changed. Reopen its inspection form before continuing.",
        ));
    }
    crate::wallet_dir::validate_name(&expected.wallet.name)?;
    if crate::address::parse(&expected.wallet.primary_address)?.net != expected.wallet.network
        || ctx.settings.node.network.net_type() != expected.wallet.network
    {
        return Err(wallet_ops::stale());
    }
    // A separate ordinary payment uncertainty is not authority to switch wallets.
    if crate::state::pending_send(ctx)?.is_some() {
        return Err(Error::safety("An ordinary payment record also remains. Inspect that record before recovering this native wallet."));
    }
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    crate::units::ensure_running(
        ctx,
        &ctx.settings.wallet.unit,
        &|| w.get_version().is_ok(),
        ctx.timeouts.unit_probe,
        cancel,
    )?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    crate::state::clear_last_open(ctx)?;
    Ok(reopen(ctx, w, &expected.wallet, &password))
}
fn with_return_state(error: Error, returned: &NativeReturn) -> Error {
    error.with_action(if returned.wallet_reopened {
        "The native process has stopped and the original wallet is open."
    } else {
        "The native process has stopped. If native uncertainty remains, use Tools → Offline → Reopen pending wallet for inspection; otherwise open the original wallet."
    })
}
impl Held {
    fn decline(mut self, w: &WalletClient, cancel: &CancelToken) -> Result<NativeReturn, Error> {
        let mut process = self.process.take().unwrap();
        let result = process
            .send(b"n\n", cancel)
            .and_then(|()| process.wait(cancel));
        drop(process);
        let mut returned = reopen(&self.ctx, w, &self.wallet, &self.password);
        if result.is_err() {
            returned.warning=Some("Review was declined and the native process was stopped. No approval was sent; check the wallet-open state.".into());
        }
        Ok(returned)
    }
}
impl HeldUnsigned {
    pub fn review(&self) -> &Review {
        &self.0.review
    }
    pub fn decline(self, w: &WalletClient, cancel: &CancelToken) -> Result<NativeReturn, Error> {
        self.0.decline(w, cancel)
    }
    pub fn sign(
        mut self,
        w: &WalletClient,
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<SignedArtifact, Error> {
        let held = &mut self.0;
        let mut process = held.process.take().unwrap();
        let result = (|| -> Result<(ArtifactReceipt, Vec<String>), Error> {
            process.send(b"y\n", cancel)?;
            process.wait(cancel)?;
            let text = std::str::from_utf8(process.output.as_slice())
                .map_err(|_| Error::backend("Monero emitted invalid signing output."))?;
            let marker = "Transaction successfully signed to file signed_monero_tx, txid ";
            let tail = text
                .split_once(marker)
                .map(|(_, s)| s.lines().next().unwrap_or(""))
                .ok_or_else(|| Error::backend("Monero did not confirm signing the artifact."))?;
            let ids: Vec<String> = tail.split(", ").map(str::trim).map(str::to_owned).collect();
            validate_ids(&ids, held.review.transactions)?;
            if !process.success() {
                return Err(Error::backend(
                    "The signing process did not finish successfully.",
                ));
            }
            let bytes = held.stage.read_generated("signed_monero_tx")?;
            if !bytes.starts_with(b"Monero signed tx set") {
                return Err(Error::backend(
                    "Monero did not create a native signed transaction artifact.",
                ));
            }
            Ok((artifacts::write_new(destination, &bytes)?, ids))
        })();
        drop(process);
        let returned = reopen(&held.ctx, w, &held.wallet, &held.password);
        match result {
            Ok((artifact, txids)) => Ok(SignedArtifact {
                artifact,
                txids,
                returned,
            }),
            Err(error) => Err(with_return_state(error, &returned)),
        }
    }
}
impl HeldSigned {
    pub fn review(&self) -> &Review {
        &self.0.review
    }
    pub fn decline(self, w: &WalletClient, cancel: &CancelToken) -> Result<NativeReturn, Error> {
        self.0.decline(w, cancel)
    }
    /// Consumes the held immutable engine review. CLI callers may pass "send" only
    /// after --yes; TUI callers require the typed final confirmation after reveal.
    pub fn submit(
        mut self,
        w: &WalletClient,
        confirmation: &str,
        max_fee: Option<Piconero>,
        cancel: &CancelToken,
    ) -> Result<Submitted, Error> {
        if confirmation != "send" {
            let returned = self.0.decline(w, cancel)?;
            return Err(with_return_state(
                Error::safety("Type send after reviewing every recipient, amount and fee."),
                &returned,
            ));
        }
        if max_fee.is_some_and(|max| self.0.review.fee > max) {
            let returned = self.0.decline(w, cancel)?;
            return Err(with_return_state(
                Error::safety("The reviewed fee exceeds the selected maximum.")
                    .with_reason("fee_above_max"),
                &returned,
            ));
        }
        let held = &mut self.0;
        let mut process = held.process.take().unwrap();
        let mut record = PendingSubmission {
            wallet: held.wallet.clone(),
            artifact_sha256: held.review.artifact_sha256.clone(),
            transactions: held.review.transactions,
            unknown_txids: held.review.transactions,
            proven_txids: vec![],
            attempted_at: crate::now_unix(),
        };
        // Persist unknown count/digest before the first and only approval write.
        if let Err(error) = process
            .pump(cancel)
            .and_then(|()| save_pending(&held.ctx, &record))
        {
            drop(process);
            let returned = reopen(&held.ctx, w, &held.wallet, &held.password);
            return Err(with_return_state(error, &returned));
        }
        // Exclude the held review/prompt from receipt parsing. Only output after
        // this checkpoint can acknowledge the approval we are about to write.
        let receipt_start = process.output.as_slice().len();
        let mut evidence_valid = true;
        let sent = process.send(b"y\n", cancel);
        let mut warning=sent.as_ref().err().map(|_|"Approval delivery was not confirmed. Some payments may have been submitted; never retry this artifact automatically.".to_string());
        if sent.is_ok() {
            loop {
                let step = process.pump(cancel);
                match capture_proven(&process.output.as_slice()[receipt_start..], &mut record) {
                    Ok(changed) => {
                        if changed {
                            if let Err(e) = save_pending(&held.ctx, &record) {
                                warning =
                                    Some(format!("Submission evidence could not be saved: {e}"));
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        evidence_valid = false;
                        warning = Some(e.to_string());
                        break;
                    }
                }
                if let Err(e) = step {
                    warning = Some(format!("Submission stopped without a complete result: {e}"));
                    break;
                }
                if process.exited() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let all_submitted = evidence_valid && record.unknown_txids == 0 && process.success();
        if !all_submitted && warning.is_none() {
            warning=Some("Monero did not prove every submission. The unresolved record blocks another payment; inspect history before clearing it.".into());
        }
        drop(process);
        let returned = reopen(&held.ctx, w, &held.wallet, &held.password);
        // Keep even fully proven hashes pending until the reopened engine sees them.
        if all_submitted && returned.wallet_reopened {
            let observed = (|| -> Result<bool, Error> {
                let accounts = w
                    .get_accounts()?
                    .into_iter()
                    .map(|a| a.account_index)
                    .collect::<Vec<_>>();
                all_hashes_observed(&accounts, &record.proven_txids, |id, account| {
                    if cancel.is_cancelled() {
                        return Err(Error::Interrupted);
                    }
                    Ok(w.get_transfer_by_txid(id, account)?
                        .is_some_and(|tx| tx.txid == id))
                })
            })();
            match observed {
                Ok(true) => if let Err(e) = remove_pending(&held.ctx) {
                    warning = Some(format!("Payments were submitted, but the pending record could not be cleared: {e}"));
                },
                Ok(false) => warning = Some("Payments were submitted, but the wallet has not observed every proven hash. The pending guard remains; inspect history before acknowledging it.".into()),
                Err(e) => warning = Some(format!("Payments were submitted, but wallet observation failed: {e} The pending guard remains; do not resubmit.")),
            }
        } else if all_submitted {
            warning = Some("Payments were submitted, but the wallet could not be reopened for observation. The pending guard remains; use the pending-wallet inspection action.".into());
        }
        Ok(Submitted {
            txids: record.proven_txids,
            unresolved: record.unknown_txids,
            all_submitted,
            returned,
            warning,
        })
    }
}
fn all_hashes_observed(
    accounts: &[u32],
    ids: &[String],
    mut lookup: impl FnMut(&str, u32) -> Result<bool, Error>,
) -> Result<bool, Error> {
    if accounts.is_empty() || ids.is_empty() {
        return Err(Error::upstream(
            "Wallet observation returned no accounts or transaction identifiers.",
        ));
    }
    let accounts = accounts
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut all_found = true;
    for id in ids {
        let mut found = false;
        for account in &accounts {
            if lookup(id, *account)? {
                found = true;
                break;
            }
        }
        all_found &= found;
    }
    Ok(all_found)
}
fn validate_ids(ids: &[String], count: u32) -> Result<(), Error> {
    if ids.len() != count as usize
        || ids
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
    {
        return Err(Error::upstream(
            "Monero returned invalid transaction identifiers.",
        ));
    }
    Ok(())
}
fn capture_proven(bytes: &[u8], record: &mut PendingSubmission) -> Result<bool, Error> {
    const PREFIX: &str = "Transaction successfully submitted, transaction ";
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::upstream("Monero emitted invalid submission evidence."))?;
    let mut ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for part in text.split_inclusive('\n') {
        // A pipe read can stop in the middle of a token or before its line end.
        // Wait for the next pump; a process exit without the newline is not proof.
        let Some(line) = part.strip_suffix('\n') else {
            break;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(token) = line.strip_prefix(PREFIX) else {
            if line.contains(PREFIX) {
                return Err(Error::upstream(
                    "Monero emitted malformed submission evidence.",
                ));
            }
            continue;
        };
        // crypto::hash's stream operator uses to_hex::formatted: <64 hex>.
        // Bare hashes, additional tokens and overlong/unterminated hashes refuse.
        let id = token
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| {
                Error::upstream("Monero emitted an invalid submitted transaction identifier.")
            })?
            .to_ascii_lowercase();
        // Seeing the same complete transcript on the next pump is idempotent;
        // two receipt lines for one hash within that transcript are not two sends.
        if !seen.insert(id.clone()) {
            return Err(Error::upstream(
                "Monero reported a duplicate submitted transaction identifier.",
            ));
        }
        if seen.len() > record.transactions as usize {
            return Err(Error::safety(
                "Monero reported more submissions than were reviewed.",
            ));
        }
        if !record
            .proven_txids
            .iter()
            .any(|x| x.eq_ignore_ascii_case(&id))
        {
            ids.push(id);
        }
    }
    if ids.len() > record.unknown_txids as usize {
        return Err(Error::safety(
            "Monero reported more submissions than were reviewed.",
        ));
    }
    // Commit this parse only after every complete receipt line is valid. Earlier
    // durably recorded hashes survive malformed later output without fabrication.
    let changed = !ids.is_empty();
    record.unknown_txids -= ids.len() as u32;
    record.proven_txids.extend(ids);
    Ok(changed)
}
fn pending_path(ctx: &Context) -> PathBuf {
    ctx.paths.state_dir.join("native-pending.json")
}
pub fn pending(ctx: &Context) -> Result<Option<PendingSubmission>, Error> {
    let path = pending_path(ctx);
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(pending_error()),
    };
    let metadata = file.metadata().map_err(|_| pending_error())?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(pending_error());
    }
    let mut bytes = vec![];
    file.by_ref()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| pending_error())?;
    let record: PendingSubmission = serde_json::from_slice(&bytes).map_err(|_| pending_error())?;
    if record.transactions == 0
        || record.transactions > 1024
        || record.unknown_txids as usize + record.proven_txids.len() != record.transactions as usize
    {
        return Err(pending_error());
    }
    if record.artifact_sha256.len() != 64
        || !record
            .artifact_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(pending_error());
    }
    validate_ids(
        &record.proven_txids,
        record.transactions - record.unknown_txids,
    )
    .map_err(|_| pending_error())?;
    Ok(Some(record))
}
fn pending_error() -> Error {
    Error::safety("A native submission record is unreadable or unresolved. Payments may already have gone out; inspect history before clearing it.").with_reason("pending_send").with_hint("fmonero wallet tools offline pending")
}
fn save_pending(ctx: &Context, record: &PendingSubmission) -> Result<(), Error> {
    ctx.paths.ensure_dirs()?;
    crate::write_private(
        &pending_path(ctx),
        &serde_json::to_vec(record).map_err(Error::from)?,
    )?;
    File::open(&ctx.paths.state_dir)
        .and_then(|f| f.sync_all())
        .map_err(|_| pending_error())
}
fn remove_pending(ctx: &Context) -> Result<(), Error> {
    match std::fs::remove_file(pending_path(ctx)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(pending_error()),
    };
    File::open(&ctx.paths.state_dir)
        .and_then(|f| f.sync_all())
        .map_err(|_| pending_error())
}
/// Explicit manual escape hatch, including a corrupt record. Never retries a payment.
pub fn clear_pending(ctx: &Context, confirmation: &str) -> Result<(), Error> {
    if confirmation != "clear" {
        return Err(Error::safety(
            "Inspect wallet history, then type clear to acknowledge unresolved submissions.",
        ));
    }
    let _lock = crate::send::take_send_lock(ctx)?;
    remove_pending(ctx)
}

fn parse_review(prompt: &str, network: crate::NetType) -> Result<Review, Error> {
    let bad = || {
        Error::safety("Monero's transaction review was not in the expected complete format. Nothing was approved.")
    };
    if prompt.len() > 32 * 1024
        || prompt.chars().any(char::is_control)
        || !prompt.ends_with("Is this okay?")
    {
        return Err(bad());
    }
    let (count, rest) = prompt
        .strip_prefix("Loaded ")
        .ok_or_else(bad)?
        .split_once(" transactions, for ")
        .ok_or_else(bad)?;
    let transactions: u32 = count.parse().map_err(|_| bad())?;
    if transactions == 0 || transactions > 1024 {
        return Err(bad());
    }
    let (input, rest) = rest.split_once(", fee ").ok_or_else(bad)?;
    let (fee, rest) = rest.split_once(", ").ok_or_else(bad)?;
    let (details, ring) = rest.split_once(", with min ring size ").ok_or_else(bad)?;
    let (ring, _) = ring.split_once(", ").ok_or_else(bad)?;
    let minimum_ring_size: u32 = ring.parse().map_err(|_| bad())?;
    if minimum_ring_size == 0 {
        return Err(bad());
    }
    let mut precision = None;
    let mut amount = |s: &str| native_money(s, &mut precision).map_err(|_| bad());
    let input_amount = amount(input)?;
    let fee = amount(fee)?;
    let mut recipients = vec![];
    let mut change_amount = Piconero(0);
    let mut change_address = None;
    let mut saw_change = false;
    for field in details.split(", ") {
        if let Some(to) = field.strip_prefix("sending ") {
            let (value, address) = to.split_once(" to ").ok_or_else(bad)?;
            // Integrated address annotations are actual Monero review text, retained
            // in engine_review; the leading address is the exact payment destination.
            let address = address.split_whitespace().next().ok_or_else(bad)?;
            if crate::address::parse(address)?.net != network {
                return Err(bad());
            }
            let amount = amount(value)?;
            if amount.0 == 0 {
                return Err(bad());
            }
            recipients.push(crate::cold::Recipient {
                address: address.into(),
                amount,
            });
        } else if field == "no change" {
            if saw_change {
                return Err(bad());
            }
            saw_change = true;
        } else if let Some((value, address)) = field.split_once(" change to ") {
            if saw_change || crate::address::parse(address)?.net != network {
                return Err(bad());
            }
            saw_change = true;
            change_amount = amount(value)?;
            change_address = Some(address.into());
        } else if field == "with no destinations" {
        } else if let Some(n) = field.strip_suffix(" dummy output(s)") {
            if n.parse::<u32>().is_err() {
                return Err(bad());
            }
        } else {
            return Err(bad());
        }
    }
    if !saw_change {
        return Err(bad());
    }
    let total = recipients
        .iter()
        .try_fold(fee.checked_add(change_amount)?, |total, r| {
            total.checked_add(r.amount)
        })?;
    if total != input_amount {
        return Err(bad());
    }
    Ok(Review {
        artifact_sha256: String::new(),
        transactions,
        input_amount,
        fee,
        recipients,
        change_amount,
        change_address,
        minimum_ring_size,
        engine_review: prompt.into(),
    })
}

/// Monero print_money inserts the wallet's saved decimal point into an atomic
/// integer without dropping any digits. It is not necessarily an XMR decimal.
fn native_money(token: &str, precision: &mut Option<usize>) -> Result<Piconero, Error> {
    let bad = || Error::safety("Monero returned an invalid or inconsistent money denomination.");
    let (whole, fraction) = token
        .split_once('.')
        .map_or((token, None), |(w, f)| (w, Some(f)));
    let places = fraction.map_or(0, str::len);
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()))
        || ![0, 3, 6, 9, 12].contains(&places)
        || precision.is_some_and(|p| p != places)
    {
        return Err(bad());
    }
    let atomic = token
        .bytes()
        .filter(|b| *b != b'.')
        .try_fold(0u64, |n, digit| {
            n.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
        })
        .ok_or_else(bad)?;
    *precision = Some(places);
    Ok(Piconero(atomic))
}

#[cfg(test)]
mod tests {
    use super::*;
    const ADDRESS:&str="888tNkZrPN6JsEgekjMnABU4TBzc2Dt29EPAvkRxbANsAnjyPbb3iQ1YBRk1UXcdRsiKc9dhwMVgN5S9cQUiyoogDavup3H";
    #[test]
    fn every_native_display_denomination_preserves_atomic_amount_fee_and_change() {
        // Literal Monero print_money forms: XMR, milli, micro, nano and pico.
        for [input, fee, recipient, change] in [
            [
                "1.100000000002",
                "0.100000000000",
                "1.000000000001",
                "0.000000000001",
            ],
            [
                "1100.000000002",
                "100.000000000",
                "1000.000000001",
                "0.000000001",
            ],
            [
                "1100000.000002",
                "100000.000000",
                "1000000.000001",
                "0.000001",
            ],
            ["1100000000.002", "100000000.000", "1000000000.001", "0.001"],
            ["1100000000002", "100000000000", "1000000000001", "1"],
        ] {
            let text = format!("Loaded 1 transactions, for {input}, fee {fee}, sending {recipient} to {ADDRESS}, {change} change to {ADDRESS}, with min ring size 16, no payment ID. Is this okay?");
            let review = parse_review(&text, crate::NetType::Mainnet).unwrap();
            assert_eq!(review.input_amount, Piconero(1_100_000_000_002));
            assert_eq!(review.fee, Piconero(100_000_000_000));
            assert_eq!(review.recipients[0].amount, Piconero(1_000_000_000_001));
            assert_eq!(review.change_amount, Piconero(1));
            assert_eq!(
                review.engine_review, text,
                "retain Monero's exact review text"
            );
        }
    }
    #[test]
    fn native_review_rejects_mixed_precision_malformed_tokens_and_overflow() {
        let valid = format!("Loaded 1 transactions, for 1.100000000001, fee 0.100000000000, sending 1.000000000001 to {ADDRESS}, no change, with min ring size 16, no payment ID. Is this okay?");
        // Each replacement represents the same fee in a different display unit.
        // Mixing it with twelve-place amounts must still be rejected.
        for fee in [
            "100.000000000",
            "100000.000000",
            "100000000.000",
            "100000000000",
        ] {
            assert!(parse_review(
                &valid.replace("0.100000000000", fee),
                crate::NetType::Mainnet
            )
            .is_err());
        }
        for token in [
            "",
            "-1",
            "+1",
            "1e3",
            "1.",
            ".001",
            "1.00",
            "1.0000",
            "1.0000000000000",
            "1.000.000",
            "1_000",
            " 1000",
            "١٠٠٠",
            "18446744073709551616",
        ] {
            assert!(
                native_money(token, &mut None).is_err(),
                "accepted malformed token {token:?}"
            );
        }
        assert_eq!(
            native_money("18446744073709551615", &mut None).unwrap(),
            Piconero(u64::MAX)
        );
        let overflow = format!("Loaded 1 transactions, for 18446744073709551615, fee 1, sending 18446744073709551615 to {ADDRESS}, no change, with min ring size 16, no payment ID. Is this okay?");
        assert!(parse_review(&overflow, crate::NetType::Mainnet).is_err());
    }
    #[test]
    fn native_reconciliation_observes_every_hash_in_actual_accounts() {
        let ids = vec!["a".repeat(64), "b".repeat(64)];
        let mut calls = vec![];
        assert!(all_hashes_observed(&[0, 1, 4, 1], &ids, |id, account| {
            calls.push((id.to_owned(), account));
            Ok((id == ids[0] && account == 1) || (id == ids[1] && account == 4))
        })
        .unwrap());
        assert_eq!(
            calls,
            vec![
                (ids[0].clone(), 0),
                (ids[0].clone(), 1),
                (ids[1].clone(), 0),
                (ids[1].clone(), 1),
                (ids[1].clone(), 4)
            ]
        );
    }
    #[test]
    fn native_reconciliation_reports_observation_failure_even_after_a_missing_hash() {
        let ids = vec!["a".repeat(64), "b".repeat(64)];
        assert!(!all_hashes_observed(
            &[0, 3],
            &ids,
            |id, account| Ok(id == ids[1] && account == 3)
        )
        .unwrap());
        let result = all_hashes_observed(&[0, 3], &ids, |id, _| {
            if id == ids[1] {
                Err(Error::upstream("observation unavailable"))
            } else {
                Ok(false)
            }
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("observation unavailable"));
        assert!(all_hashes_observed(&[], &ids, |_, _| panic!("no accounts")).is_err());
        assert!(all_hashes_observed(&[0], &[], |_, _| panic!("no hashes")).is_err());
    }
    #[test]
    fn exact_engine_review_binds_amounts_fee_change_and_rejects_partial_or_injected_prompts() {
        let text=format!("Loaded 2 transactions, for 1.100000000001, fee 0.100000000000, sending 1.000000000001 to {ADDRESS}, no change, with min ring size 16, no payment ID. Is this okay?");
        let review = parse_review(&text, crate::NetType::Mainnet).unwrap();
        assert_eq!(review.transactions, 2);
        assert_eq!(review.fee, Piconero(100_000_000_000));
        assert_eq!(review.recipients[0].amount, Piconero(1_000_000_000_001));
        assert!(parse_review(
            &text.replace("fee 0.100", "fee 0.200"),
            crate::NetType::Mainnet
        )
        .is_err());
        assert!(parse_review(
            text.trim_end_matches("Is this okay?"),
            crate::NetType::Mainnet
        )
        .is_err());
        assert!(parse_review(
            &text.replace("sending", "\x1b]52;payload\x07sending"),
            crate::NetType::Mainnet
        )
        .is_err());
    }
    fn submission_record(transactions: u32) -> PendingSubmission {
        let wallet = WalletIdentity {
            name: "scratch".into(),
            network: crate::NetType::Mainnet,
            primary_address: ADDRESS.into(),
        };
        PendingSubmission {
            wallet,
            artifact_sha256: "a".repeat(64),
            transactions,
            unknown_txids: transactions,
            proven_txids: vec![],
            attempted_at: 0,
        }
    }

    const SUBMISSION_RECEIPTS: &str =
        include_str!("../../../tests/fixtures/native/submit-monero-0.18.5.3.stdout");

    #[test]
    fn partial_submission_counts_only_proven_unique_hashes() {
        let first_end = SUBMISSION_RECEIPTS.find('\n').unwrap() + 1;
        let mut record = submission_record(2);
        // Every incomplete split, including a complete <hash> without a newline,
        // must keep the full uncertainty count.
        for end in 0..first_end {
            assert!(!capture_proven(&SUBMISSION_RECEIPTS.as_bytes()[..end], &mut record).unwrap());
            assert_eq!(record.unknown_txids, 2);
            assert!(record.proven_txids.is_empty());
        }
        let first = &SUBMISSION_RECEIPTS[..first_end];
        assert!(capture_proven(first.as_bytes(), &mut record).unwrap());
        assert_eq!(record.proven_txids, vec!["0123456789abcdef".repeat(4)]);
        assert_eq!(record.unknown_txids, 1);
        assert!(!capture_proven(first.as_bytes(), &mut record).unwrap());
        let partial = format!("{first}Error: second transaction was refused\n");
        assert!(!capture_proven(partial.as_bytes(), &mut record).unwrap());
        assert_eq!(record.unknown_txids, 1);
        assert!(capture_proven(SUBMISSION_RECEIPTS.as_bytes(), &mut record).unwrap());
        assert_eq!(record.unknown_txids, 0);
        assert_eq!(record.proven_txids[1], "fedcba9876543210".repeat(4));
        assert!(!capture_proven(SUBMISSION_RECEIPTS.as_bytes(), &mut record).unwrap());
    }

    #[test]
    fn submission_receipts_refuse_malformed_delimiters_tokens_and_overlong_hashes() {
        let hash = "0123456789abcdef".repeat(4);
        for token in [
            hash.clone(),
            format!("<{hash}"),
            format!("{hash}>"),
            format!("<<{hash}>>"),
            format!("<{}>", &hash[..63]),
            format!("<{hash}0>"),
            format!("<{hash}>0"),
            format!("<{hash}> extra"),
            format!("<{hash}> "),
            format!("<{}g>", &hash[..63]),
            format!("<{}é>", &hash[..63]),
        ] {
            let line = format!("Transaction successfully submitted, transaction {token}\n");
            let mut record = submission_record(2);
            assert!(
                capture_proven(line.as_bytes(), &mut record).is_err(),
                "{token}"
            );
            assert_eq!(record.unknown_txids, 2);
            assert!(record.proven_txids.is_empty());
        }
        let mut record = submission_record(2);
        let embedded = format!("not a receipt: {}", SUBMISSION_RECEIPTS);
        assert!(capture_proven(embedded.as_bytes(), &mut record).is_err());
        assert!(record.proven_txids.is_empty());
    }

    #[test]
    fn submission_receipts_refuse_duplicate_lines_and_more_than_reviewed() {
        let first_end = SUBMISSION_RECEIPTS.find('\n').unwrap() + 1;
        let first = &SUBMISSION_RECEIPTS[..first_end];
        let mut record = submission_record(2);
        let duplicate = format!("{first}{first}");
        assert!(capture_proven(duplicate.as_bytes(), &mut record).is_err());
        assert_eq!(record.unknown_txids, 2);
        assert!(record.proven_txids.is_empty());
        let mut record = submission_record(1);
        assert!(capture_proven(SUBMISSION_RECEIPTS.as_bytes(), &mut record).is_err());
        assert_eq!(record.unknown_txids, 1);
        assert!(record.proven_txids.is_empty());
        // A malformed later batch cannot discard an earlier proven/persisted hash.
        let mut record = submission_record(2);
        assert!(capture_proven(first.as_bytes(), &mut record).unwrap());
        assert!(capture_proven(duplicate.as_bytes(), &mut record).is_err());
        assert_eq!(record.unknown_txids, 1);
        assert_eq!(record.proven_txids, vec!["0123456789abcdef".repeat(4)]);
        let upper_hash = first.replace("0123456789abcdef", "0123456789ABCDEF");
        assert!(capture_proven(format!("{first}{upper_hash}").as_bytes(), &mut record).is_err());
        assert_eq!(record.unknown_txids, 1);
    }
    #[test]
    fn pending_record_blocks_ordinary_payments_and_explicit_clear_is_required() {
        let path = std::env::temp_dir().join(format!(
            "fmonero-native-pending-{}-{}",
            std::process::id(),
            crate::rpc::random_hex(8).unwrap()
        ));
        let ctx = Context::for_test(&path).unwrap();
        let wallet = WalletIdentity {
            name: "scratch".into(),
            network: crate::NetType::Mainnet,
            primary_address: ADDRESS.into(),
        };
        let record = PendingSubmission {
            wallet,
            artifact_sha256: "a".repeat(64),
            transactions: 2,
            unknown_txids: 2,
            proven_txids: vec![],
            attempted_at: 0,
        };
        save_pending(&ctx, &record).unwrap();
        assert!(crate::send::refuse_if_send_in_flight(&ctx).is_err());
        assert!(clear_pending(&ctx, "yes").is_err());
        clear_pending(&ctx, "clear").unwrap();
        assert!(pending(&ctx).unwrap().is_none());
        std::fs::remove_dir_all(path).unwrap();
    }
}
