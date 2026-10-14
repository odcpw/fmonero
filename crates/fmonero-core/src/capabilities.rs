//! Observed backend version and wallet creation provenance. Monero decides support.
use crate::{
    rpc::jsonrpc::Deadline,
    wallet_ops::{self, WalletIdentity},
    Context, Error, WalletClient,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletKind {
    Unknown,
    Deterministic,
    ViewOnly,
    Ledger,
    Trezor,
}
#[derive(Clone, Debug, Serialize)]
pub struct Capabilities {
    pub wallet: WalletIdentity,
    pub rpc_major: u16,
    pub rpc_minor: u16,
    pub release: bool,
    pub kind_hint: WalletKind,
    pub spend_authority: Option<bool>,
    pub backend_decides: bool,
    pub native_cli_available: bool,
}
/// kind_hint is provenance stored by successful fmonero creation, not a key probe.
/// Never use it to authorize spending or claim a hardware device is connected.
pub fn observe(ctx: &Context, w: &WalletClient) -> Result<Capabilities, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    let wallet = wallet_ops::identity(ctx, w)?;
    let (rpc_major, rpc_minor, release) = w.get_version()?;
    #[derive(Deserialize)]
    struct Reply {
        value: String,
    }
    let reply: Result<Reply, Error> = w.call(
        "get_attribute",
        serde_json::json!({"key":"fmonero.kind"}),
        Deadline::Probe,
    );
    let kind_hint = match reply.as_ref().map(|r| r.value.as_str()) {
        Ok("deterministic") => WalletKind::Deterministic,
        Ok("view_only") => WalletKind::ViewOnly,
        Ok("ledger") => WalletKind::Ledger,
        Ok("trezor") => WalletKind::Trezor,
        _ => WalletKind::Unknown,
    };
    Ok(Capabilities {
        wallet,
        rpc_major,
        rpc_minor,
        release,
        kind_hint,
        spend_authority: None,
        backend_decides: true,
        native_cli_available: crate::proc::which("monero-wallet-cli").is_some(),
    })
}
pub(crate) fn note(w: &WalletClient, kind: WalletKind) -> Result<(), Error> {
    let value = match kind {
        WalletKind::Unknown => "unknown",
        WalletKind::Deterministic => "deterministic",
        WalletKind::ViewOnly => "view_only",
        WalletKind::Ledger => "ledger",
        WalletKind::Trezor => "trezor",
    };
    w.ack(
        "set_attribute",
        serde_json::json!({"key":"fmonero.kind","value":value}),
        Deadline::Probe,
    )
}
