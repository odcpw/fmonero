use clap::Subcommand;
use fmonero_core::{
    cold, native_tx, send, wallet_ops, CancelToken, Context, Error, Piconero, SendRequest,
    WalletClient,
};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum OfflineCmd {
    /// Prepare an unsigned artifact in a view-only wallet; never relays
    Prepare {
        to: String,
        amount: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        account: Option<u32>,
        #[arg(long)]
        priority: Option<u32>,
        #[arg(long)]
        max_fee: Option<String>,
    },
    /// Describe an unsigned artifact through an eligible Monero wallet
    Describe { path: PathBuf },
    /// Review then sign offline. Without --yes, decline and reopen the wallet
    Sign {
        path: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        secret_stdin: bool,
    },
    /// Review then submit a signed artifact. Without --yes, decline; never retries
    Submit {
        path: PathBuf,
        #[arg(long)]
        secret_stdin: bool,
        #[arg(long)]
        max_fee: Option<String>,
    },
    /// Read unresolved native submissions locally, with no RPC
    Pending,
    /// Reopen only the recorded wallet to inspect transfers; retains the payment guard
    InspectPending {
        #[arg(long)]
        secret_stdin: bool,
    },
    /// After checking actual transfers, explicitly clear a native uncertainty record
    ClearPending,
}

fn absolute(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()
            .map_err(|_| Error::failure("Could not resolve the artifact directory."))?
            .join(path))
    }
}

pub fn run(
    ctx: &Context,
    cmd: OfflineCmd,
    yes: bool,
    cancel: &CancelToken,
) -> Result<(serde_json::Value, i32), Error> {
    match cmd {
        OfflineCmd::Pending => {
            return Ok((serde_json::json!({"pending": native_tx::pending(ctx)?}), 0))
        }
        OfflineCmd::InspectPending { secret_stdin } => {
            if !yes {
                return Err(Error::safety("Review the pending record, then repeat with --yes to reopen that wallet for inspection. Payments remain blocked."));
            }
            let expected = native_tx::pending(ctx)?
                .ok_or_else(|| Error::usage("There is no native submission record to inspect."))?;
            let password =
                crate::prompt::secret("Recorded wallet password: ", false, secret_stdin)?;
            let refresh = ctx.clone().with_wallet_probe_at_refresh();
            let wallet = WalletClient::from_ctx(&refresh)?;
            let returned = native_tx::reopen_pending_for_inspection(
                &refresh, &wallet, &expected, password, cancel,
            )?;
            let code = if returned.wallet_reopened { 0 } else { 7 };
            return Ok((
                serde_json::json!({"pending": expected, "returned": returned, "inspection_only": true}),
                code,
            ));
        }
        OfflineCmd::ClearPending => {
            if !yes {
                return Err(Error::safety("Check the actual transfers before clearing uncertainty. Repeat with --yes after review."));
            }
            native_tx::clear_pending(ctx, "clear")?;
            return Ok((serde_json::json!({"native_pending_cleared": true}), 0));
        }
        _ => {}
    }
    let refreshed_context = ctx.clone().with_wallet_probe_at_refresh();
    let ctx = &refreshed_context;
    let wallet = WalletClient::from_ctx(ctx)?;
    let identity = wallet_ops::identity(ctx, &wallet)?;
    match cmd {
        OfflineCmd::Prepare {
            to,
            amount,
            output,
            account,
            priority,
            max_fee,
        } => {
            if !yes {
                return Err(Error::safety("Review the recipient and unsigned artifact destination, then repeat with --yes. This operation never relays."));
            }
            let request = SendRequest {
                dest: to,
                amount: Piconero::parse(&amount)?,
                account_index: account.unwrap_or(ctx.settings.wallet.account),
                priority: priority.unwrap_or(ctx.settings.wallet.priority),
                note: None,
                max_fee: max_fee.as_deref().map(Piconero::parse).transpose()?,
            };
            let node = if ctx.settings.node.mode.is_remote() {
                None
            } else {
                fmonero_core::NodeClient::from_ctx(ctx).ok()
            };
            let gate = match &node {
                Some(node) => send::NodeGate::Local(node),
                None if ctx.settings.node.mode.is_remote() => send::NodeGate::Remote,
                None => send::NodeGate::Unreachable,
            };
            Ok((
                serde_json::to_value(cold::prepare_unsigned(
                    ctx,
                    &wallet,
                    gate,
                    &identity,
                    &request,
                    &absolute(&output)?,
                    cancel,
                )?)?,
                0,
            ))
        }
        OfflineCmd::Describe { path } => Ok((
            serde_json::to_value(cold::describe_unsigned(
                ctx,
                &wallet,
                &identity,
                &absolute(&path)?,
            )?)?,
            0,
        )),
        OfflineCmd::Sign {
            path,
            output,
            secret_stdin,
        } => {
            let password = crate::prompt::secret("Offline wallet password: ", false, secret_stdin)?;
            let held = native_tx::review_unsigned(
                ctx,
                &wallet,
                &identity,
                password,
                &absolute(&path)?,
                cancel,
            )?;
            let review = serde_json::to_value(held.review())?;
            if !yes {
                let returned = held.decline(&wallet, cancel)?;
                return Ok((
                    serde_json::json!({"review": review, "signed": false,
                    "confirmation_required": true, "returned": returned}),
                    3,
                ));
            }
            let result = held.sign(&wallet, &absolute(&output)?, cancel)?;
            Ok((
                serde_json::json!({"review": review, "signed": true, "result": result}),
                0,
            ))
        }
        OfflineCmd::Submit {
            path,
            secret_stdin,
            max_fee,
        } => {
            let password =
                crate::prompt::secret("Submitting wallet password: ", false, secret_stdin)?;
            let maximum = max_fee.as_deref().map(Piconero::parse).transpose()?;
            let held = native_tx::review_signed(
                ctx,
                &wallet,
                &identity,
                password,
                &absolute(&path)?,
                cancel,
            )?;
            let review = serde_json::to_value(held.review())?;
            if !yes {
                let returned = held.decline(&wallet, cancel)?;
                return Ok((
                    serde_json::json!({"review": review, "submitted": false,
                    "confirmation_required": true, "returned": returned}),
                    3,
                ));
            }
            let result = held.submit(&wallet, "send", maximum, cancel)?;
            let code = if result.all_submitted { 0 } else { 7 };
            Ok((
                serde_json::json!({"review": review, "result": result}),
                code,
            ))
        }
        OfflineCmd::Pending | OfflineCmd::InspectPending { .. } | OfflineCmd::ClearPending => {
            unreachable!()
        }
    }
}
