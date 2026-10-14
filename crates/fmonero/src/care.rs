use clap::{Subcommand, ValueEnum};
use fmonero_core::{
    capabilities, hardware, history::HistoryQuery, history_export, maintenance, recovery,
    wallet_ops, watch, CancelToken, Context, Error, WalletClient,
};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum CareCmd {
    /// Ask the wallet engine which capabilities can currently be established
    Capabilities,
    /// Rebuild wallet history; --hard discards transaction metadata before rebuilding
    Rescan {
        #[arg(long, conflicts_with = "hard")]
        spent: bool,
        #[arg(long)]
        hard: bool,
    },
    /// Read old and new passwords privately, then change wallet encryption
    Password {
        #[arg(long)]
        secret_stdin: bool,
    },
    /// Create a new encrypted recovery wallet; labels/notes/contacts are not backed up
    Backup {
        directory: PathBuf,
        #[arg(long, default_value_t = 0)]
        restore_height: u64,
        #[arg(long)]
        secret_stdin: bool,
    },
    /// Create an incoming-only wallet. Input lines: private view key, new password
    ViewOnly {
        name: String,
        address: String,
        #[arg(long, default_value_t = 0)]
        restore_height: u64,
        #[arg(long)]
        secret_stdin: bool,
    },
    /// Create a wallet through Monero's Ledger or Trezor engine
    Hardware {
        name: String,
        #[arg(value_enum)]
        device: Device,
        #[arg(long, default_value_t = 0)]
        restore_height: u64,
        #[arg(long)]
        secret_stdin: bool,
    },
    /// Export encrypted output information to a new private exchange artifact
    ExportOutputs {
        path: PathBuf,
    },
    ImportOutputs {
        path: PathBuf,
    },
    ExportKeyImages {
        path: PathBuf,
    },
    /// Requires an explicitly trusted daemon
    ImportKeyImages {
        path: PathBuf,
    },
    /// Export exact deterministic history into a new private file
    ExportHistory {
        path: PathBuf,
        #[arg(long, value_enum, default_value = "csv")]
        format: Format,
        #[arg(long)]
        account: Option<u32>,
        #[arg(long, default_value_t = 100000)]
        limit: usize,
        #[arg(long)]
        cached: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Device {
    Ledger,
    Trezor,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    Csv,
    Json,
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
    cmd: CareCmd,
    yes: bool,
    cancel: &CancelToken,
) -> Result<(serde_json::Value, i32), Error> {
    let approved = || {
        if yes {
            Ok(())
        } else {
            Err(Error::safety(
                "Review this wallet operation and repeat it with --yes.",
            ))
        }
    };
    let wallet = WalletClient::from_ctx(ctx)?;
    let value = match cmd {
        CareCmd::Capabilities => serde_json::to_value(capabilities::observe(ctx, &wallet)?)?,
        CareCmd::Rescan { spent, hard } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            let kind = if spent {
                maintenance::RescanKind::Spent
            } else {
                maintenance::RescanKind::Blockchain { hard }
            };
            maintenance::rescan(ctx, &wallet, &identity, kind)?;
            serde_json::json!({"rescan_requested": true, "complete_sync": false, "hard": hard, "spent": spent})
        }
        CareCmd::Password { secret_stdin } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            let old = crate::prompt::secret("Current wallet password: ", false, secret_stdin)?;
            let new = crate::prompt::secret("New wallet password: ", true, secret_stdin)?;
            maintenance::change_password(ctx, &wallet, &identity, old, new)?;
            serde_json::json!({"password_changed": true})
        }
        CareCmd::Backup {
            directory,
            restore_height,
            secret_stdin,
        } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            let password = crate::prompt::secret("Recovery wallet password: ", true, secret_stdin)?;
            serde_json::to_value(recovery::create(
                ctx,
                &wallet,
                &identity,
                password,
                restore_height,
                &absolute(&directory)?,
                cancel,
            )?)?
        }
        CareCmd::ViewOnly {
            name,
            address,
            restore_height,
            secret_stdin,
        } => {
            approved()?;
            fmonero_core::wallet_dir::require_new(ctx, &name)?;
            let view_key = crate::prompt::secret("Private view key: ", false, secret_stdin)?;
            let password = crate::prompt::secret("New wallet password: ", true, secret_stdin)?;
            let fresh = ctx.clone().with_wallet_probe_at_refresh();
            let wallet = WalletClient::from_ctx(&fresh)?;
            serde_json::to_value(watch::create(
                &fresh,
                &wallet,
                watch::CreateViewOnly {
                    name,
                    address,
                    view_key,
                    password,
                    restore_height,
                },
                cancel,
            )?)?
        }
        CareCmd::Hardware {
            name,
            device,
            restore_height,
            secret_stdin,
        } => {
            approved()?;
            fmonero_core::wallet_dir::require_new(ctx, &name)?;
            let password =
                crate::prompt::secret("Device wallet file password: ", true, secret_stdin)?;
            let device = match device {
                Device::Ledger => hardware::Device::Ledger,
                Device::Trezor => hardware::Device::Trezor,
            };
            let fresh = ctx.clone().with_wallet_probe_at_refresh();
            let wallet = WalletClient::from_ctx(&fresh)?;
            serde_json::to_value(hardware::create(
                &fresh,
                &wallet,
                hardware::CreateHardware {
                    name,
                    password,
                    device,
                    restore_height,
                },
                cancel,
            )?)?
        }
        CareCmd::ExportOutputs { path } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            serde_json::to_value(watch::export_outputs(
                ctx,
                &wallet,
                &identity,
                &absolute(&path)?,
            )?)?
        }
        CareCmd::ImportOutputs { path } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            serde_json::json!({"outputs_imported": watch::import_outputs(ctx, &wallet, &identity, &absolute(&path)?)?})
        }
        CareCmd::ExportKeyImages { path } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            serde_json::to_value(watch::export_key_images(
                ctx,
                &wallet,
                &identity,
                &absolute(&path)?,
            )?)?
        }
        CareCmd::ImportKeyImages { path } => {
            approved()?;
            let identity = wallet_ops::identity(ctx, &wallet)?;
            serde_json::to_value(watch::import_key_images(
                ctx,
                &wallet,
                &identity,
                &absolute(&path)?,
            )?)?
        }
        CareCmd::ExportHistory {
            path,
            format,
            account,
            limit,
            cached,
        } => {
            approved()?;
            if limit == 0 || limit > 100000 {
                return Err(Error::usage("Export between 1 and 100000 history rows."));
            }
            let mut query = HistoryQuery {
                account_index: account.unwrap_or(ctx.settings.wallet.account),
                limit,
                cached,
                ..Default::default()
            };
            let format = match format {
                Format::Csv => history_export::ExportFormat::Csv,
                Format::Json => history_export::ExportFormat::Json,
            };
            serde_json::to_value(history_export::export(
                ctx,
                if cached { None } else { Some(&wallet) },
                &mut query,
                format,
                &absolute(&path)?,
            )?)?
        }
    };
    let code = if matches!(value["state"].as_str(), Some("unknown" | "needs_setup")) {
        7
    } else {
        0
    };
    Ok((value, code))
}
