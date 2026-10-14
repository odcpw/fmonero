//! Typed advanced operations. Forms construct these jobs; only core performs wallet work.
use super::{hit::Button, screens, Modal, Msg, Tui};
use fmonero_core::{
    proofs, wallet_ops::WalletIdentity, CancelToken, Context, Error, Piconero, SecretString,
    WalletClient,
};
use ftui::layout::Rect;
use ftui::widgets::{paragraph::Paragraph, Widget};
use ftui::{Cmd, Frame, KeyCode, KeyEvent};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    SignMessage,
    VerifyMessage,
    TxProof,
    VerifyTxProof,
    SpendProof,
    VerifySpendProof,
    ReserveProof,
    VerifyReserveProof,
    ExportHistory,
    Rescan,
    ChangePassword,
    CreateViewOnly,
    ExportOutputs,
    ImportOutputs,
    ExportKeyImages,
    ImportKeyImages,
    RecoveryBackup,
    CreateHardware,
    Capabilities,
    PrepareUnsigned,
    InspectUnsigned,
    SignUnsigned,
    SubmitSigned,
    NativePending,
    ReopenNativePending,
    ClearNativePending,
    Transport,
}
impl Tool {
    pub fn title(self) -> &'static str {
        match self {
            Self::SignMessage => "Sign message",
            Self::VerifyMessage => "Verify message",
            Self::TxProof => "Create transaction proof",
            Self::VerifyTxProof => "Verify transaction proof",
            Self::SpendProof => "Create spend proof",
            Self::VerifySpendProof => "Verify spend proof",
            Self::ReserveProof => "Create reserve proof",
            Self::VerifyReserveProof => "Verify reserve proof",
            Self::ExportHistory => "Export payment history",
            Self::Rescan => "Rescan wallet",
            Self::ChangePassword => "Change wallet password",
            Self::CreateViewOnly => "Create view-only wallet",
            Self::ExportOutputs => "Export outputs",
            Self::ImportOutputs => "Import outputs",
            Self::ExportKeyImages => "Export key images",
            Self::ImportKeyImages => "Import key images",
            Self::RecoveryBackup => "Encrypted recovery backup",
            Self::CreateHardware => "Create hardware wallet",
            Self::Capabilities => "Wallet and backend capabilities",
            Self::PrepareUnsigned => "Prepare unsigned payment",
            Self::InspectUnsigned => "Inspect unsigned artifact",
            Self::SignUnsigned => "Review and sign offline",
            Self::SubmitSigned => "Review and submit signed artifact",
            Self::NativePending => "Inspect unresolved submission",
            Self::ReopenNativePending => "Reopen pending wallet for inspection",
            Self::ClearNativePending => "Acknowledge unresolved submission",
            Self::Transport => "Node TLS, proxy and trust",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
        Self::SignMessage => "Sign text using a wallet address; no private key is exported.",
        Self::VerifyMessage => "Check a message, address and signature together.",
        Self::TxProof => "Prove a payment to its recipient without exporting transaction keys.",
        Self::VerifyTxProof => "Check recipient, received amount and confirmation evidence.",
        Self::SpendProof => "Prove that this wallet spent a transaction.",
        Self::VerifySpendProof => "Check a transaction's spend proof and optional message.",
        Self::ReserveProof => "All scope reveals whole-wallet reserves. Account scope reveals the requested amount.",
        Self::VerifyReserveProof => "Check a reserve proof's total and already-spent amounts.",
        Self::ExportHistory => "Write a new private JSON or spreadsheet-safe CSV artifact.",
        Self::Rescan => "Regular rebuilds history; hard discards history first. Spent requires a trusted node.",
        Self::ChangePassword => "Encrypt the open wallet with a new password. An uncertain outcome must not be retried automatically.",
        Self::CreateViewOnly => "Track receipts without spend authority. Import key images to learn what has been spent.",
        Self::ExportOutputs => "Move observed outputs to your offline signing wallet in a new private artifact.",
        Self::ImportOutputs => "Load a matching wallet's observed outputs into your offline signing wallet.",
        Self::ExportKeyImages => "Move signed spend-state evidence back to your view-only wallet.",
        Self::ImportKeyImages => "Update view-only spent amounts from matching signed key images. Requires a trusted node.",
        Self::RecoveryBackup => "Monero creates an encrypted recovery wallet. Labels, contacts, notes and local history metadata are not included.",
        Self::CreateHardware => "Connect and unlock your device; open its Monero app. Follow device prompts. Existing wallet files are never replaced.",
        Self::Capabilities => "Inspect creation provenance and backend version. Only Monero can decide actual signing and device support.",
        Self::PrepareUnsigned => "Use a view-only wallet to prepare a native unsigned payment artifact. Nothing is broadcast.",
        Self::InspectUnsigned => "Ask Monero to describe a native unsigned artifact; no signing or submission.",
        Self::SignUnsigned => "Load an immutable unsigned artifact into offline Monero; approve its exact review to sign.",
        Self::SubmitSigned => "Monero connects to the configured node before review. Typed send approves the exact held signed artifact once.",
        Self::NativePending => "Inspect the durable artifact digest, proven transaction IDs and unknown submission count without wallet RPC.",
        Self::ReopenNativePending => "Open only the wallet recorded in native uncertainty to inspect history. Confirmation may start its service. The pending guard stays; nothing is sent or cleared.",
        Self::ClearNativePending => "Inspect wallet history first. Clearing uncertainty does not cancel a payment or prove that it failed.",
        Self::Transport => "Set connection policy for the selected node. Required TLS never downgrades. Trust only a daemon you control.",
    }
    }
    pub fn needs_identity(self) -> bool {
        matches!(
            self,
            Self::SignMessage
                | Self::TxProof
                | Self::SpendProof
                | Self::ReserveProof
                | Self::ExportHistory
                | Self::Rescan
                | Self::ChangePassword
                | Self::ExportOutputs
                | Self::ImportOutputs
                | Self::ExportKeyImages
                | Self::ImportKeyImages
                | Self::RecoveryBackup
                | Self::Capabilities
                | Self::PrepareUnsigned
                | Self::InspectUnsigned
                | Self::SignUnsigned
                | Self::SubmitSigned
        )
    }
    pub fn fields(self) -> Vec<(&'static str, bool, &'static str)> {
        match self {
            Self::Transport => vec![
                ("TLS: auto / required / disabled", false, "auto"),
                ("CA file (optional, absolute)", false, ""),
                ("Certificate fingerprint (optional)", false, ""),
                ("SOCKS proxy IP:port (optional)", false, ""),
                ("Trust remote daemon: true / false", false, "false"),
            ],
            Self::PrepareUnsigned => vec![
                ("Recipient address", false, ""),
                ("Amount XMR", false, ""),
                ("Account", false, "0"),
                ("Priority: 1 / 2 / 3 / 4", false, "1"),
                ("Fee ceiling XMR (optional)", false, ""),
                ("New unsigned file (absolute)", false, ""),
            ],
            Self::InspectUnsigned => vec![("Unsigned input file (absolute)", false, "")],
            Self::SignUnsigned => vec![
                ("Unsigned input file (absolute)", false, ""),
                ("Current wallet password", true, ""),
                ("New signed output file (absolute)", false, ""),
            ],
            Self::SubmitSigned => vec![
                ("Signed input file (absolute)", false, ""),
                ("Current wallet password", true, ""),
                ("Fee ceiling XMR (optional)", false, ""),
            ],
            Self::NativePending => vec![],
            Self::ReopenNativePending => vec![("Recorded wallet password", true, "")],
            Self::ClearNativePending => {
                vec![("Type clear after inspecting wallet history", false, "")]
            }
            Self::RecoveryBackup => vec![
                ("New recovery directory (absolute)", false, ""),
                ("Recovery wallet password", true, ""),
                ("Password again", true, ""),
                ("Restore height", false, "0"),
            ],
            Self::CreateHardware => vec![
                ("New wallet name", false, ""),
                ("Device: ledger / trezor", false, "ledger"),
                ("Wallet password", true, ""),
                ("Password again", true, ""),
                ("Restore height", false, "0"),
            ],
            Self::Capabilities => vec![("Inspection scope", false, "Current wallet and backend")],
            Self::Rescan => vec![("Rescan: regular / hard / spent", false, "regular")],
            Self::ChangePassword => vec![
                ("Current password", true, ""),
                ("New password", true, ""),
                ("New password again", true, ""),
            ],
            Self::CreateViewOnly => vec![
                ("New wallet name", false, ""),
                ("Standard primary address", false, ""),
                ("Private view key", true, ""),
                ("Wallet password", true, ""),
                ("Password again", true, ""),
                ("Restore height", false, "0"),
            ],
            Self::ExportOutputs | Self::ExportKeyImages => {
                vec![("New output file (absolute)", false, "")]
            }
            Self::ImportOutputs | Self::ImportKeyImages => {
                vec![("Input artifact (absolute)", false, "")]
            }
            Self::SignMessage => vec![
                ("Message", false, ""),
                ("Account", false, "0"),
                ("Address index", false, "0"),
                ("Signature type: spend / view", false, "spend"),
                ("Output file (optional)", false, ""),
            ],
            Self::VerifyMessage => vec![
                ("Address", false, ""),
                ("Message", false, ""),
                ("Signature", false, ""),
            ],
            Self::TxProof => vec![
                ("Transaction ID", false, ""),
                ("Recipient address", false, ""),
                ("Message (optional)", false, ""),
                ("Output file (optional)", false, ""),
            ],
            Self::VerifyTxProof => vec![
                ("Transaction ID", false, ""),
                ("Recipient address", false, ""),
                ("Message (optional)", false, ""),
                ("Proof", false, ""),
            ],
            Self::SpendProof => vec![
                ("Transaction ID", false, ""),
                ("Message (optional)", false, ""),
                ("Output file (optional)", false, ""),
            ],
            Self::VerifySpendProof => vec![
                ("Transaction ID", false, ""),
                ("Message (optional)", false, ""),
                ("Proof", false, ""),
            ],
            Self::ReserveProof => vec![
                ("Scope: account / all", false, "account"),
                ("Account", false, "0"),
                ("Amount XMR (account scope)", false, ""),
                ("Message (optional)", false, ""),
                ("Output file (optional)", false, ""),
            ],
            Self::VerifyReserveProof => vec![
                ("Address", false, ""),
                ("Message (optional)", false, ""),
                ("Proof", false, ""),
            ],
            Self::ExportHistory => vec![
                ("New output file (absolute)", false, ""),
                ("Format: csv / json", false, "csv"),
                ("Account", false, "0"),
            ],
        }
    }
}

pub(super) enum Job {
    Transport {
        expected: fmonero_core::settings::NodeSettings,
        values: Vec<String>,
    },
    PrepareUnsigned {
        wallet: WalletIdentity,
        request: fmonero_core::SendRequest,
        path: PathBuf,
    },
    InspectUnsigned {
        wallet: WalletIdentity,
        path: PathBuf,
    },
    NativePending,
    ClearNativePending(String),
    RecoveryBackup {
        wallet: WalletIdentity,
        password: SecretString,
        height: u64,
        path: PathBuf,
    },
    CreateHardware(fmonero_core::hardware::CreateHardware),
    Capabilities,
    Rescan {
        wallet: WalletIdentity,
        kind: fmonero_core::maintenance::RescanKind,
    },
    ChangePassword {
        wallet: WalletIdentity,
        old: SecretString,
        new: SecretString,
    },
    CreateViewOnly(fmonero_core::watch::CreateViewOnly),
    Exchange {
        wallet: WalletIdentity,
        tool: Tool,
        path: PathBuf,
    },
    Sign {
        wallet: WalletIdentity,
        account: u32,
        index: u32,
        kind: proofs::SignatureType,
        data: String,
        output: Option<PathBuf>,
    },
    VerifyMessage {
        address: String,
        data: String,
        signature: String,
    },
    TxProof {
        wallet: WalletIdentity,
        transaction: String,
        address: String,
        message: String,
        output: Option<PathBuf>,
    },
    VerifyTx {
        transaction: String,
        address: String,
        message: String,
        proof: String,
    },
    Spend {
        wallet: WalletIdentity,
        transaction: String,
        message: String,
        output: Option<PathBuf>,
    },
    VerifySpend {
        transaction: String,
        message: String,
        proof: String,
    },
    Reserve {
        wallet: WalletIdentity,
        scope: proofs::ReserveScope,
        message: String,
        output: Option<PathBuf>,
    },
    VerifyReserve {
        address: String,
        message: String,
        proof: String,
    },
    Export {
        wallet: WalletIdentity,
        path: PathBuf,
        format: fmonero_core::history_export::ExportFormat,
        account: u32,
    },
}

pub(super) struct ResultView {
    pub title: String,
    pub lines: Vec<String>,
    pub copy: Option<String>,
    pub failed: bool,
    pub scroll: usize,
    pub max_scroll: std::cell::Cell<usize>,
}
impl ResultView {
    pub fn complete(title: &str, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
            copy: None,
            failed: false,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        }
    }
    fn artifact(title: &str, receipt: fmonero_core::artifacts::ArtifactReceipt) -> Self {
        Self::complete(
            title,
            vec![
                receipt.path.display().to_string(),
                format!("{} bytes\nSHA-256 {}", receipt.bytes, receipt.sha256),
            ],
        )
    }
    pub fn restored(result: fmonero_core::restore::Restored) -> Self {
        use fmonero_core::restore::RestoreState;
        let (title, message, failed) = match result.state {
            RestoreState::Ready => (
                "Wallet ready",
                "Monero created the wallet and configured its node.",
                false,
            ),
            RestoreState::NeedsSetup => (
                "Wallet needs setup",
                "The wallet exists. Open it explicitly to finish setup; do not create it again.",
                true,
            ),
            RestoreState::Unknown => (
                "Creation outcome unknown",
                "Inspect the wallet list before deciding what to do. Do not retry automatically.",
                true,
            ),
        };
        let mut lines = vec![message.into(), result.name];
        if let Some(address) = result.address {
            lines.push(address);
        }
        if let Some(warning) = result.warning {
            lines.push(warning);
        }
        Self {
            title: title.into(),
            lines,
            failed,
            copy: None,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        }
    }
    fn checked(title: &str, good: bool, lines: Vec<String>) -> Self {
        let mut all = vec![if good {
            "Verified by Monero.".into()
        } else {
            "Verification failed. Do not rely on this proof.".into()
        }];
        all.extend(lines);
        Self {
            title: title.into(),
            lines: all,
            copy: None,
            failed: !good,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        }
    }
    fn signature(signature: proofs::Signature, output: Option<PathBuf>) -> Result<Self, Error> {
        let mut lines = vec!["Created by Monero. Share only with the intended recipient.".into()];
        if let Some(path) = output {
            let receipt =
                fmonero_core::artifacts::write_new(&path, signature.signature.as_bytes())?;
            lines.push(format!(
                "Saved {} bytes to {}\nSHA-256 {}",
                receipt.bytes,
                receipt.path.display(),
                receipt.sha256
            ));
        }
        lines.push(signature.signature.clone());
        Ok(Self {
            title: "Proof / signature created".into(),
            lines,
            copy: Some(signature.signature),
            failed: false,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        })
    }
}

impl Job {
    pub fn execute(self, ctx: &Context, cancel: &CancelToken) -> Result<ResultView, Error> {
        match &self {
            Self::Transport { expected, values } => {
                let _lock = fmonero_core::send::take_send_lock(ctx)?;
                fmonero_core::send::refuse_if_send_in_flight(ctx)?;
                let mut fresh = ctx.clone();
                fresh.settings = fmonero_core::Settings::load(&ctx.paths)?;
                if &fresh.settings.node != expected {
                    return Err(Error::safety(
                        "The node settings changed. Reopen this form before applying a policy.",
                    ));
                }
                fresh.settings.set("node.ca_file", "")?;
                fresh.settings.set("node.fingerprint", "")?;
                for (key, value) in [
                    "node.tls",
                    "node.ca_file",
                    "node.fingerprint",
                    "node.proxy",
                    "node.remote_trusted",
                ]
                .into_iter()
                .zip(values)
                {
                    fresh.settings.set(key, value.trim())?;
                }
                fmonero_core::transport::DaemonConnection::from_context(&fresh)?;
                fresh.settings.save(&fresh)?;
                let mut result = ResultView::complete(
                    "Node connection policy saved",
                    vec!["The policy is saved for subsequent connections.".into()],
                );
                let apply = (|| -> Result<bool, Error> {
                    if !fmonero_core::app::status(&fresh)?.wallet_running
                        || fmonero_core::state::last_open(&fresh)?.is_none()
                    {
                        return Ok(false);
                    }
                    let w = WalletClient::from_ctx(&fresh)?;
                    fmonero_core::transport::configure(&fresh, &w)?;
                    Ok(true)
                })();
                match apply {
                    Ok(true) => result
                        .lines
                        .push("Monero accepted the live connection policy.".into()),
                    Ok(false) => result.lines.push(
                        "No open wallet was contacted. Opening one will use the saved policy."
                            .into(),
                    ),
                    Err(error) => {
                        result.failed = true;
                        result.lines.push(format!("Saved, but live application was not confirmed: {error} Reopen the wallet explicitly to apply it."));
                    }
                }
                return Ok(result);
            }
            Self::NativePending => {
                return Ok(match fmonero_core::native_tx::pending(ctx)? {
                    None => ResultView::complete(
                        "Native submission status",
                        vec!["No unresolved native submission is recorded.".into()],
                    ),
                    Some(pending) => {
                        let mut result = ResultView::complete(
                            "Unresolved native submission",
                            vec![
                                pending.wallet.name,
                                format!(
                                    "Artifact SHA-256 {}\nTransactions {}\nUnknown IDs {}",
                                    pending.artifact_sha256,
                                    pending.transactions,
                                    pending.unknown_txids
                                ),
                                "Inspect wallet history. If closed, use Tools → Offline → Reopen pending wallet for inspection. Do not retry this artifact automatically."
                                    .into(),
                            ],
                        );
                        result.lines.extend(pending.proven_txids);
                        result.failed = true;
                        result
                    }
                });
            }
            Self::ClearNativePending(confirmation) => {
                fmonero_core::native_tx::clear_pending(ctx, confirmation)?;
                return Ok(ResultView::complete("Unresolved record acknowledged", vec!["The record was cleared. This did not cancel or resend a payment, or prove any previous failure.".into()]));
            }
            _ => {}
        }
        let operation_ctx = if matches!(&self, Self::CreateViewOnly(_) | Self::CreateHardware(_)) {
            ctx.clone().with_wallet_probe_at_refresh()
        } else {
            ctx.clone()
        };
        let ctx = &operation_ctx;
        let w = WalletClient::from_ctx(ctx)?;
        if matches!(
            &self,
            Self::VerifyMessage { .. }
                | Self::VerifyTx { .. }
                | Self::VerifySpend { .. }
                | Self::VerifyReserve { .. }
        ) {
            // This worker exists only after explicit form review/confirmation. Verification
            // needs Monero's public check API, never an identity lookup or private wallet open.
            fmonero_core::units::ensure_running(
                ctx,
                &ctx.settings.wallet.unit,
                &|| w.get_version().is_ok(),
                ctx.timeouts.unit_probe,
                cancel,
            )?;
        }
        match self {
            Self::Transport { .. } | Self::NativePending | Self::ClearNativePending(_) => {
                unreachable!("local operation returned above")
            }
            Self::PrepareUnsigned {
                wallet,
                request,
                path,
            } => {
                let n = if ctx.settings.node.mode.is_remote() {
                    None
                } else {
                    Some(fmonero_core::NodeClient::from_ctx(ctx)?)
                };
                let gate = n.as_ref().map_or(
                    fmonero_core::send::NodeGate::Remote,
                    fmonero_core::send::NodeGate::Local,
                );
                let r = fmonero_core::cold::prepare_unsigned(
                    ctx, &w, gate, &wallet, &request, &path, cancel,
                )?;
                Ok(ResultView::complete("Unsigned artifact prepared", vec![r.artifact.path.display().to_string(), format!("SHA-256 {}\nTransactions {}\nRecipient {}\nAmount {} XMR\nFee {} XMR", r.artifact.sha256, r.transactions, r.destination, r.amount.to_xmr_min(12), r.fee.to_xmr_min(12)), "Nothing was broadcast. Review and sign this artifact on your offline signing wallet.".into()]))
            }
            Self::InspectUnsigned { wallet, path } => {
                let r = fmonero_core::cold::describe_unsigned(ctx, &w, &wallet, &path)?;
                let mut lines = vec![format!("SHA-256 {}", r.artifact_sha256)];
                for (i, tx) in r.transactions.into_iter().enumerate() {
                    lines.push(format!("Transaction {}\nInput {} XMR\nOutput {} XMR\nFee {} XMR\nChange {} XMR\nChange address {}\nRing size {} · unlock time {}", i + 1, tx.amount_in.to_xmr_min(12), tx.amount_out.to_xmr_min(12), tx.fee.to_xmr_min(12), tx.change_amount.to_xmr_min(12), tx.change_address, tx.ring_size, tx.unlock_time));
                    lines.push(format!(
                        "Payment ID {}\nWeight {}",
                        if tx.payment_id.is_empty() {
                            "none"
                        } else {
                            &tx.payment_id
                        },
                        tx.weight
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".into())
                    ));
                    for recipient in tx.recipients {
                        lines.push(format!(
                            "{} XMR to\n{}",
                            recipient.amount.to_xmr_min(12),
                            recipient.address
                        ));
                    }
                }
                Ok(ResultView::complete("Unsigned artifact description", lines))
            }
            Self::RecoveryBackup {
                wallet,
                password,
                height,
                path,
            } => {
                let r = fmonero_core::recovery::create(
                    ctx, &w, &wallet, password, height, &path, cancel,
                )?;
                Ok(ResultView::complete(
                    "Recovery wallet created",
                    vec![
                        r.wallet_file.display().to_string(),
                        r.address,
                        format!(
                            "Restore height {} · password reopen verified: {}",
                            r.restore_height, r.password_reopened
                        ),
                        r.coverage,
                    ],
                ))
            }
            Self::CreateHardware(request) => {
                fmonero_core::hardware::create(ctx, &w, request, cancel).map(ResultView::restored)
            }
            Self::Capabilities => {
                let c = fmonero_core::capabilities::observe(ctx, &w)?;
                use fmonero_core::capabilities::WalletKind;
                let kind = match c.kind_hint {
                    WalletKind::Unknown => "Unknown",
                    WalletKind::Deterministic => "Deterministic",
                    WalletKind::ViewOnly => "View-only",
                    WalletKind::Ledger => "Ledger",
                    WalletKind::Trezor => "Trezor",
                };
                Ok(ResultView::complete("Wallet capabilities", vec![format!("Creation provenance: {kind}\nRPC {}.{} · release {}\nNative Monero CLI available: {}", c.rpc_major, c.rpc_minor, c.release, c.native_cli_available), "Provenance does not prove usable spend authority or that a device is connected. Monero decides support when the operation runs.".into(), "For view-only wallets, spent status is unknown without matching key images. Unlocked receipt evidence is separate from signing capability.".into()]))
            }
            Self::Rescan { wallet, kind } => {
                fmonero_core::maintenance::rescan(ctx, &w, &wallet, kind)?;
                Ok(ResultView::complete("Rescan completed", vec!["Monero completed the requested rescan. Refresh balances and history to see current evidence.".into()]))
            }
            Self::ChangePassword { wallet, old, new } => {
                fmonero_core::maintenance::change_password(ctx, &w, &wallet, old, new)?;
                Ok(ResultView::complete(
                    "Password changed",
                    vec![
                        "Monero confirmed the new wallet password. The input buffers were cleared."
                            .into(),
                    ],
                ))
            }
            Self::CreateViewOnly(request) => {
                fmonero_core::watch::create(ctx, &w, request, cancel).map(ResultView::restored)
            }
            Self::Exchange { wallet, tool, path } => {
                use fmonero_core::watch;
                Ok(match tool {
                    Tool::ExportOutputs => ResultView::artifact(
                        "Outputs exported",
                        watch::export_outputs(ctx, &w, &wallet, &path)?,
                    ),
                    Tool::ExportKeyImages => ResultView::artifact(
                        "Key images exported",
                        watch::export_key_images(ctx, &w, &wallet, &path)?,
                    ),
                    Tool::ImportOutputs => ResultView::complete(
                        "Outputs imported",
                        vec![format!(
                            "Monero imported {} outputs.",
                            watch::import_outputs(ctx, &w, &wallet, &path)?
                        )],
                    ),
                    Tool::ImportKeyImages => {
                        let r = watch::import_key_images(ctx, &w, &wallet, &path)?;
                        ResultView::complete(
                            "Key images imported",
                            vec![format!(
                                "Height {}\nSpent {} XMR\nUnspent {} XMR",
                                r.height,
                                r.spent.to_xmr_min(12),
                                r.unspent.to_xmr_min(12)
                            )],
                        )
                    }
                    _ => return Err(Error::usage("This operation is not an exchange operation.")),
                })
            }
            Self::Sign {
                wallet,
                account,
                index,
                kind,
                data,
                output,
            } => ResultView::signature(
                proofs::sign_message(ctx, &w, &wallet, account, index, kind, &data)?,
                output,
            ),
            Self::VerifyMessage {
                address,
                data,
                signature,
            } => {
                let r = proofs::verify_message(&w, &address, &data, &signature)?;
                Ok(ResultView::checked(
                    "Message verification",
                    r.good,
                    vec![format!(
                        "Version {} · {} · legacy {}",
                        r.version, r.signature_type, r.old
                    )],
                ))
            }
            Self::TxProof {
                wallet,
                transaction,
                address,
                message,
                output,
            } => ResultView::signature(
                proofs::transaction_proof(ctx, &w, &wallet, &transaction, &address, &message)?,
                output,
            ),
            Self::VerifyTx {
                transaction,
                address,
                message,
                proof,
            } => {
                let r =
                    proofs::check_transaction_proof(&w, &transaction, &address, &message, &proof)?;
                Ok(ResultView::checked(
                    "Transaction proof",
                    r.good,
                    vec![format!(
                        "Received {} XMR · confirmations {} · in pool {}",
                        r.received.to_xmr_min(12),
                        r.confirmations,
                        r.in_pool
                    )],
                ))
            }
            Self::Spend {
                wallet,
                transaction,
                message,
                output,
            } => ResultView::signature(
                proofs::spend_proof(ctx, &w, &wallet, &transaction, &message)?,
                output,
            ),
            Self::VerifySpend {
                transaction,
                message,
                proof,
            } => Ok(ResultView::checked(
                "Spend proof",
                proofs::check_spend_proof(&w, &transaction, &message, &proof)?,
                vec![],
            )),
            Self::Reserve {
                wallet,
                scope,
                message,
                output,
            } => ResultView::signature(
                proofs::reserve_proof(ctx, &w, &wallet, scope, &message)?,
                output,
            ),
            Self::VerifyReserve {
                address,
                message,
                proof,
            } => {
                let r = proofs::check_reserve_proof(&w, &address, &message, &proof)?;
                Ok(ResultView::checked(
                    "Reserve proof",
                    r.good,
                    vec![format!(
                        "Total {} XMR\nSpent {} XMR",
                        r.total.to_xmr_min(12),
                        r.spent.to_xmr_min(12)
                    )],
                ))
            }
            Self::Export {
                wallet,
                path,
                format,
                account,
            } => {
                if fmonero_core::wallet_ops::identity(ctx, &w)? != wallet {
                    return Err(Error::safety("The wallet changed. Reopen the export form."));
                }
                let mut query = fmonero_core::HistoryQuery {
                    account_index: account,
                    limit: usize::MAX,
                    ..Default::default()
                };
                let r =
                    fmonero_core::history_export::export(ctx, Some(&w), &mut query, format, &path)?;
                Ok(ResultView {
                    title: "History exported".into(),
                    lines: vec![
                        r.path.display().to_string(),
                        format!("{} bytes\nSHA-256 {}", r.bytes, r.sha256),
                    ],
                    copy: None,
                    failed: false,
                    scroll: 0,
                    max_scroll: std::cell::Cell::new(0),
                })
            }
        }
    }
}

pub(super) fn job(
    tool: Tool,
    identity: Option<WalletIdentity>,
    values: &[String],
) -> Result<Job, String> {
    let get = |i: usize| values.get(i).cloned().unwrap_or_default();
    let number = |i| {
        get(i)
            .trim()
            .parse::<u32>()
            .map_err(|_| "Account and address indices must be whole numbers.".to_string())
    };
    let output = |i| {
        let s = get(i);
        (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()))
    };
    let wallet = || {
        identity
            .clone()
            .ok_or_else(|| "Open a wallet and reopen this form.".to_string())
    };
    Ok(match tool {
        Tool::Rescan => Job::Rescan {
            wallet: wallet()?,
            kind: match get(0).trim() {
                "regular" => fmonero_core::maintenance::RescanKind::Blockchain { hard: false },
                "hard" => fmonero_core::maintenance::RescanKind::Blockchain { hard: true },
                "spent" => fmonero_core::maintenance::RescanKind::Spent,
                _ => return Err("Choose regular, hard, or spent.".into()),
            },
        },
        Tool::ExportOutputs
        | Tool::ImportOutputs
        | Tool::ExportKeyImages
        | Tool::ImportKeyImages => {
            let path = PathBuf::from(get(0));
            if !path.is_absolute() {
                return Err("Enter an absolute artifact path.".into());
            }
            Job::Exchange {
                wallet: wallet()?,
                tool,
                path,
            }
        }
        Tool::PrepareUnsigned => Job::PrepareUnsigned {
            wallet: wallet()?,
            path: PathBuf::from(get(5)),
            request: fmonero_core::SendRequest {
                dest: get(0),
                amount: Piconero::parse(get(1).trim()).map_err(|e| e.to_string())?,
                account_index: number(2)?,
                priority: number(3)?,
                note: None,
                max_fee: if get(4).trim().is_empty() {
                    None
                } else {
                    Some(Piconero::parse(get(4).trim()).map_err(|e| e.to_string())?)
                },
            },
        },
        Tool::InspectUnsigned => Job::InspectUnsigned {
            wallet: wallet()?,
            path: PathBuf::from(get(0)),
        },
        Tool::NativePending => Job::NativePending,
        Tool::ClearNativePending => {
            if get(0) != "clear" {
                return Err("Inspect wallet history first, then type clear exactly.".into());
            }
            Job::ClearNativePending(get(0))
        }
        Tool::Capabilities => Job::Capabilities,
        Tool::Transport
        | Tool::ReopenNativePending
        | Tool::SignUnsigned
        | Tool::SubmitSigned
        | Tool::RecoveryBackup
        | Tool::CreateHardware
        | Tool::ChangePassword
        | Tool::CreateViewOnly => {
            return Err("Protected form fields must be moved into their typed operation.".into())
        }
        Tool::SignMessage => Job::Sign {
            wallet: wallet()?,
            account: number(1)?,
            index: number(2)?,
            kind: match get(3).trim() {
                "spend" => proofs::SignatureType::Spend,
                "view" => proofs::SignatureType::View,
                _ => return Err("Signature type must be spend or view.".into()),
            },
            data: get(0),
            output: output(4),
        },
        Tool::VerifyMessage => Job::VerifyMessage {
            address: get(0),
            data: get(1),
            signature: get(2),
        },
        Tool::TxProof => Job::TxProof {
            wallet: wallet()?,
            transaction: get(0),
            address: get(1),
            message: get(2),
            output: output(3),
        },
        Tool::VerifyTxProof => Job::VerifyTx {
            transaction: get(0),
            address: get(1),
            message: get(2),
            proof: get(3),
        },
        Tool::SpendProof => Job::Spend {
            wallet: wallet()?,
            transaction: get(0),
            message: get(1),
            output: output(2),
        },
        Tool::VerifySpendProof => Job::VerifySpend {
            transaction: get(0),
            message: get(1),
            proof: get(2),
        },
        Tool::ReserveProof => Job::Reserve {
            wallet: wallet()?,
            scope: match get(0).trim() {
                "all" => proofs::ReserveScope::All,
                "account" => proofs::ReserveScope::Account {
                    account: number(1)?,
                    amount: Piconero::parse(get(2).trim()).map_err(|e| e.to_string())?,
                },
                _ => {
                    return Err(
                        "Scope must be account or all. All discloses whole-wallet reserves.".into(),
                    )
                }
            },
            message: get(3),
            output: output(4),
        },
        Tool::VerifyReserveProof => Job::VerifyReserve {
            address: get(0),
            message: get(1),
            proof: get(2),
        },
        Tool::ExportHistory => Job::Export {
            wallet: wallet()?,
            path: PathBuf::from(get(0)),
            format: match get(1).trim() {
                "csv" => fmonero_core::history_export::ExportFormat::Csv,
                "json" => fmonero_core::history_export::ExportFormat::Json,
                _ => return Err("Choose csv or json.".into()),
            },
            account: number(2)?,
        },
    })
}

impl Tui {
    pub(super) fn result_key(&mut self, mut result: ResultView, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Escape | KeyCode::Enter => {
                self.modal = self.modal_back.take().map(|m| *m);
                return Cmd::none();
            }
            KeyCode::Down | KeyCode::PageDown => {
                result.scroll = result.scroll.saturating_add(1).min(result.max_scroll.get())
            }
            KeyCode::Up | KeyCode::PageUp => result.scroll = result.scroll.saturating_sub(1),
            KeyCode::Char('c') | KeyCode::Char('C') => {
                if let Some(value) = &result.copy {
                    self.copy(value);
                }
            }
            _ => {}
        }
        self.modal = Some(Modal::Result(result));
        Cmd::none()
    }
}
pub(super) fn result_view(m: &Tui, result: &ResultView, frame: &mut Frame) {
    let Some(inner) = screens::overlay(
        m,
        frame,
        88,
        frame.bounds().height.saturating_sub(1),
        &result.title,
    ) else {
        return;
    };
    m.hits.modal(inner);
    let lines = if m.privacy() {
        vec![if result.failed {
            "Operation requires attention. Details hidden by privacy.".into()
        } else {
            "Operation complete. Details hidden by privacy.".into()
        }]
    } else {
        result
            .lines
            .iter()
            .flat_map(|line| {
                line.lines()
                    .flat_map(|part| screens::wrap_words(part, inner.width as usize))
            })
            .collect::<Vec<_>>()
    };
    let room = inner.height.saturating_sub(2) as usize;
    result.max_scroll.set(lines.len().saturating_sub(room));
    let start = result.scroll.min(result.max_scroll.get());
    for (i, line) in lines.iter().skip(start).take(room).enumerate() {
        Paragraph::new(line.as_str())
            .style(if result.failed {
                m.theme.err()
            } else {
                m.theme.text()
            })
            .render(
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                frame,
            );
    }
    let mut buttons = vec![
        (Button::ResultUp, "↑", "Previous"),
        (Button::ResultDown, "↓", "More"),
    ];
    if result.copy.is_some() {
        buttons.push((Button::ResultCopy, "C", "Copy"));
    }
    screens::chip_row(
        m,
        Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
        &buttons,
        frame,
    );
    screens::modal_actions(m, inner, "Close", frame);
}
