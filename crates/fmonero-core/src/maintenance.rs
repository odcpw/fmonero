//! Deliberate wallet maintenance delegated to Monero under the payment guard.
use crate::{
    rpc::jsonrpc::Deadline,
    secrets::SecretRef,
    wallet_ops::{self, WalletIdentity},
    Context, Error, SecretString, WalletClient,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RescanKind {
    Spent,
    Blockchain { hard: bool },
}
#[derive(Deserialize)]
struct Ack {}
pub fn rescan(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    kind: RescanKind,
) -> Result<(), Error> {
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    match kind {
        RescanKind::Spent => {
            if !crate::transport::DaemonConnection::from_context(ctx)?.trusted() {
                return Err(Error::safety(
                    "Spent-output rescanning requires an explicitly trusted daemon.",
                ));
            }
            let _: Ack = w.protected("rescan_spent", &serde_json::json!({}), Deadline::Refresh)?;
        }
        RescanKind::Blockchain { hard } => {
            #[derive(Serialize)]
            struct Params {
                hard: bool,
            }
            let _: Ack = w.protected("rescan_blockchain", &Params { hard }, Deadline::Refresh)?;
        }
    }
    crate::state::note_refresh(ctx)
}
pub fn change_password(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    old_password: SecretString,
    new_password: SecretString,
) -> Result<(), Error> {
    if new_password.is_empty() {
        return Err(Error::usage("Choose a nonempty new wallet password."));
    }
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params<'a> {
        old_password: SecretRef<'a>,
        new_password: SecretRef<'a>,
    }
    let _: Ack = w.protected(
        "change_wallet_password",
        &Params {
            old_password: SecretRef(old_password.expose_secret()),
            new_password: SecretRef(new_password.expose_secret()),
        },
        Deadline::Open,
    )?;
    Ok(())
}
