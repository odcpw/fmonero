//! Wallet identity and the shared guard for interactive operations.
use crate::{address, Context, Error, NetType, WalletClient};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletIdentity {
    pub name: String,
    pub network: NetType,
    pub primary_address: String,
}

/// Snapshot this when opening a form, then pass it to the eventual action.
pub fn identity(ctx: &Context, wallet: &WalletClient) -> Result<WalletIdentity, Error> {
    let (name, network) = crate::state::last_open(ctx)?
        .ok_or_else(|| Error::safety("Open a wallet before using this tool."))?;
    let primary_address = wallet.get_address(0)?.primary;
    if address::parse(&primary_address)?.net != network {
        return Err(stale());
    }
    Ok(WalletIdentity {
        name,
        network,
        primary_address,
    })
}

pub(crate) fn stale() -> Error {
    Error::safety(
        "The wallet or selected item changed. Open the form again and review the current values.",
    )
    .with_reason("wallet_changed")
    .with_hint("fmonero wallet address")
}
pub(crate) fn verify(
    ctx: &Context,
    wallet: &WalletClient,
    expected: &WalletIdentity,
) -> Result<(), Error> {
    if identity(ctx, wallet)? != *expected {
        return Err(stale());
    }
    Ok(())
}
/// Keep the returned lock alive across every read/check/mutation in an operation.
pub(crate) fn guard(
    ctx: &Context,
    wallet: &WalletClient,
    expected: &WalletIdentity,
) -> Result<std::fs::File, Error> {
    let lock = crate::send::take_send_lock(ctx)?;
    crate::send::refuse_if_send_in_flight(ctx)?;
    verify(ctx, wallet, expected)?;
    Ok(lock)
}
pub(crate) fn text(value: &str) -> Result<(), Error> {
    if value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(Error::usage(
            "Use at most 1024 bytes of text without control characters.",
        ));
    }
    Ok(())
}
