use clap::Subcommand;
use fmonero_core::{accounts, Context, Error, WalletClient};

#[derive(Subcommand)]
pub enum AccountCmd {
    /// List accounts and show which account is selected
    List,
    /// Create an account with a label
    Create { label: String },
    /// Rename an existing account
    Rename { index: u32, label: String },
    /// Choose an account for subsequent wallet commands
    Select { index: u32 },
}

pub fn run(ctx: &Context, cmd: AccountCmd) -> Result<serde_json::Value, Error> {
    let wallet = WalletClient::from_ctx(ctx)?;
    let snapshot = accounts::snapshot(ctx, &wallet)?;
    match cmd {
        AccountCmd::List => Ok(serde_json::to_value(snapshot)?),
        AccountCmd::Create { label } => Ok(serde_json::to_value(accounts::create(
            ctx,
            &wallet,
            &snapshot.wallet,
            &label,
        )?)?),
        AccountCmd::Rename { index, label } => {
            let row = snapshot
                .accounts
                .iter()
                .find(|row| row.account_index == index)
                .ok_or_else(|| Error::not_found("That account does not exist."))?;
            accounts::rename(ctx, &wallet, &snapshot.wallet, row, &label)?;
            Ok(serde_json::json!({"account_index": index, "label": label, "renamed": true}))
        }
        AccountCmd::Select { index } => {
            let row = snapshot
                .accounts
                .iter()
                .find(|row| row.account_index == index)
                .ok_or_else(|| Error::not_found("That account does not exist."))?;
            accounts::select(ctx, &wallet, &snapshot.wallet, row)?;
            Ok(serde_json::json!({"account_index": index, "label": row.label, "selected": true}))
        }
    }
}
