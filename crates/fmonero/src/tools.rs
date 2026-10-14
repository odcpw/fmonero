//! CLI adapters for advanced typed wallet workflows.
use clap::Subcommand;
use fmonero_core::{artifacts, proofs, wallet_ops, CancelToken, Context, Error, WalletClient};
use std::{
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

#[derive(Subcommand)]
pub enum ToolsCmd {
    /// Prepare, describe, review/sign, submit, and recover offline transactions
    Offline {
        #[command(subcommand)]
        cmd: crate::offline::OfflineCmd,
    },
    /// Restore/import/maintenance, devices, exchange artifacts and exact history export
    Care {
        #[command(subcommand)]
        cmd: crate::care::CareCmd,
    },
    /// Sign or verify the exact bytes of a text message
    Message {
        #[command(subcommand)]
        cmd: MessageCmd,
    },
    /// Create or verify transaction, spend and reserve proofs
    Proof {
        #[command(subcommand)]
        cmd: ProofCmd,
    },
}

#[derive(Subcommand)]
pub enum MessageCmd {
    /// Sign the exact UTF-8 message file into a new private signature file
    Sign {
        data: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        account: Option<u32>,
        #[arg(long, default_value_t = 0)]
        index: u32,
        /// Sign ownership of the private view key instead of the spend key
        #[arg(long)]
        view: bool,
    },
    /// Check signature against the exact message bytes and expected address
    Verify {
        address: String,
        data: PathBuf,
        signature: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum ProofCmd {
    /// Prove payment to an address
    Transaction {
        txid: String,
        address: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        message: Option<PathBuf>,
    },
    /// Check a payment proof and report the exact received amount
    CheckTransaction {
        txid: String,
        address: String,
        signature: PathBuf,
        #[arg(long)]
        message: Option<PathBuf>,
    },
    /// Prove that this wallet spent a transaction's inputs
    Spend {
        txid: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        message: Option<PathBuf>,
    },
    CheckSpend {
        txid: String,
        signature: PathBuf,
        #[arg(long)]
        message: Option<PathBuf>,
    },
    /// Disclose a chosen reserve scope. All funds requires explicit --all
    Reserve {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        all: bool,
        #[arg(long, conflicts_with = "all")]
        amount: Option<String>,
        #[arg(long, conflicts_with = "all")]
        account: Option<u32>,
        #[arg(long)]
        message: Option<PathBuf>,
    },
    CheckReserve {
        address: String,
        signature: PathBuf,
        #[arg(long)]
        message: Option<PathBuf>,
    },
}

/// Public message/proof artifacts only; never reads a Monero wallet or key file.
fn text(ctx: &Context, path: &Path, trim_newline: bool) -> Result<String, Error> {
    let absolute = path
        .canonicalize()
        .map_err(|e| Error::io("inspect", "the text artifact", path, &e))?;
    let wallet_dir = ctx
        .settings
        .wallet
        .dir
        .canonicalize()
        .unwrap_or_else(|_| ctx.settings.wallet.dir.clone());
    if absolute.starts_with(wallet_dir)
        || path.extension().is_some_and(|ext| ext == "keys")
        || PathBuf::from(format!("{}.keys", path.display())).exists()
    {
        return Err(Error::safety(
            "Use a message or proof file outside the wallet folder.",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| Error::io("open", "the text artifact", path, &e))?;
    if !file
        .metadata()
        .map_err(|e| Error::io("inspect", "the text artifact", path, &e))?
        .is_file()
    {
        return Err(Error::safety("The text artifact must be a regular file."));
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io("read", "the text artifact", path, &e))?;
    if bytes.len() > 65536 {
        return Err(Error::usage("Message and proof text is limited to 64 KiB."));
    }
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| Error::usage("The text artifact must be UTF-8."))?;
    let value = if trim_newline {
        value
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(value)
    } else {
        value
    };
    Ok(value.to_owned())
}

fn message(ctx: &Context, path: Option<&Path>) -> Result<String, Error> {
    path.map(|p| text(ctx, p, false))
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn save(path: &Path, signature: proofs::Signature) -> Result<serde_json::Value, Error> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| Error::failure("Could not resolve the artifact directory."))?
            .join(path)
    };
    Ok(serde_json::to_value(artifacts::write_new(
        &path,
        format!("{}\n", signature.signature).as_bytes(),
    )?)?)
}

pub fn run(
    ctx: &Context,
    cmd: ToolsCmd,
    yes: bool,
    cancel: &CancelToken,
) -> Result<(serde_json::Value, i32), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let wallet = WalletClient::from_ctx(ctx)?;
    let approved = || {
        if yes {
            Ok(())
        } else {
            Err(Error::safety(
                "Review the exact message and proof scope, then repeat this command with --yes.",
            ))
        }
    };
    let mut value = match cmd {
        ToolsCmd::Offline { cmd } => return crate::offline::run(ctx, cmd, yes, cancel),
        ToolsCmd::Care { cmd } => return crate::care::run(ctx, cmd, yes, cancel),
        ToolsCmd::Message { cmd } => match cmd {
            MessageCmd::Sign {
                data,
                output,
                account,
                index,
                view,
            } => {
                approved()?;
                let identity = wallet_ops::identity(ctx, &wallet)?;
                let kind = if view {
                    proofs::SignatureType::View
                } else {
                    proofs::SignatureType::Spend
                };
                let signature = proofs::sign_message(
                    ctx,
                    &wallet,
                    &identity,
                    account.unwrap_or(ctx.settings.wallet.account),
                    index,
                    kind,
                    &text(ctx, &data, false)?,
                )?;
                save(&output, signature)?
            }
            MessageCmd::Verify {
                address,
                data,
                signature,
            } => serde_json::to_value(proofs::verify_message(
                &wallet,
                &address,
                &text(ctx, &data, false)?,
                &text(ctx, &signature, true)?,
            )?)?,
        },
        ToolsCmd::Proof { cmd } => match cmd {
            ProofCmd::Transaction {
                txid,
                address,
                output,
                message: note,
            } => {
                approved()?;
                let identity = wallet_ops::identity(ctx, &wallet)?;
                save(
                    &output,
                    proofs::transaction_proof(
                        ctx,
                        &wallet,
                        &identity,
                        &txid,
                        &address,
                        &message(ctx, note.as_deref())?,
                    )?,
                )?
            }
            ProofCmd::CheckTransaction {
                txid,
                address,
                signature,
                message: note,
            } => serde_json::to_value(proofs::check_transaction_proof(
                &wallet,
                &txid,
                &address,
                &message(ctx, note.as_deref())?,
                &text(ctx, &signature, true)?,
            )?)?,
            ProofCmd::Spend {
                txid,
                output,
                message: note,
            } => {
                approved()?;
                let identity = wallet_ops::identity(ctx, &wallet)?;
                save(
                    &output,
                    proofs::spend_proof(
                        ctx,
                        &wallet,
                        &identity,
                        &txid,
                        &message(ctx, note.as_deref())?,
                    )?,
                )?
            }
            ProofCmd::CheckSpend {
                txid,
                signature,
                message: note,
            } => serde_json::json!({"good":
                proofs::check_spend_proof(&wallet, &txid, &message(ctx, note.as_deref())?, &text(ctx, &signature, true)?)?}),
            ProofCmd::Reserve {
                output,
                all,
                amount,
                account,
                message: note,
            } => {
                approved()?;
                let scope = if all {
                    proofs::ReserveScope::All
                } else {
                    proofs::ReserveScope::Account {
                        account: account.unwrap_or(ctx.settings.wallet.account),
                        amount: fmonero_core::Piconero::parse(amount.as_deref().ok_or_else(
                            || {
                                Error::usage(
                                    "Choose --all or an exact --amount for the reserve proof.",
                                )
                            },
                        )?)?,
                    }
                };
                let identity = wallet_ops::identity(ctx, &wallet)?;
                save(
                    &output,
                    proofs::reserve_proof(
                        ctx,
                        &wallet,
                        &identity,
                        scope,
                        &message(ctx, note.as_deref())?,
                    )?,
                )?
            }
            ProofCmd::CheckReserve {
                address,
                signature,
                message: note,
            } => serde_json::to_value(proofs::check_reserve_proof(
                &wallet,
                &address,
                &message(ctx, note.as_deref())?,
                &text(ctx, &signature, true)?,
            )?)?,
        },
    };
    for key in ["received", "total", "spent"] {
        if let Some(amount) = value.get(key).and_then(serde_json::Value::as_u64) {
            value[format!("{key}_pico")] = serde_json::json!(amount);
            value[format!("{key}_xmr")] =
                serde_json::json!(fmonero_core::Piconero(amount).to_xmr_string());
        }
    }
    let code = if value.get("good").and_then(serde_json::Value::as_bool) == Some(false) {
        1
    } else {
        0
    };
    Ok((value, code))
}
