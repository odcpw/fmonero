//! Deterministic restoration through Monero, with guarded ownership and protected input.
use crate::{
    address, session::wallet_session, CancelToken, Context, Error, NetType, SecretString,
    WalletClient,
};
use serde::Serialize;

pub struct RestoreRequest {
    pub name: String,
    pub seed: SecretString,
    pub password: SecretString,
    pub seed_offset: Option<SecretString>,
    pub restore_height: u64,
    pub language: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestorePhase {
    StartingBackend,
    Restoring,
    ConfiguringNode,
    Ready,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestoreState {
    Ready,
    NeedsSetup,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct Restored {
    pub name: String,
    pub restore_height: u64,
    pub address: Option<String>,
    pub network: Option<NetType>,
    pub state: RestoreState,
    pub warning: Option<String>,
    pub unit_started: bool,
}

pub fn restore(
    ctx: &Context,
    w: &WalletClient,
    request: RestoreRequest,
    cancel: &CancelToken,
) -> Result<Restored, Error> {
    restore_with_progress(ctx, w, request, cancel, &|_| {})
}

pub fn restore_with_progress(
    ctx: &Context,
    w: &WalletClient,
    request: RestoreRequest,
    cancel: &CancelToken,
    progress: &dyn Fn(RestorePhase),
) -> Result<Restored, Error> {
    crate::wallet_dir::require_new(ctx, &request.name)?;
    if request.seed.is_empty() {
        return Err(Error::usage(
            "Enter the wallet's recovery seed in the protected field.",
        ));
    }
    if request.password.is_empty() {
        return Err(Error::usage(
            "Choose a nonempty password for the restored wallet.",
        ));
    }
    if let Some(language) = &request.language {
        crate::settings::validate_seed_language(language)?;
    }
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let _lock = crate::send::take_send_lock(ctx)?;
    crate::send::refuse_if_send_in_flight(ctx)?;
    progress(RestorePhase::StartingBackend);
    let unit_started = crate::units::ensure_running(
        ctx,
        &ctx.settings.wallet.unit,
        &|| w.get_version().is_ok(),
        ctx.timeouts.unit_probe,
        cancel,
    )?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    // Recheck under the lock immediately before dispatch. Monero is the final
    // no-overwrite authority; fmonero never creates/deletes wallet files itself.
    crate::wallet_dir::require_new(ctx, &request.name)?;
    progress(RestorePhase::Restoring);
    crate::state::clear_last_open(ctx)?;
    let response = w.restore_protected(&request);
    let mut restored = Restored {
        name: request.name.clone(),
        restore_height: request.restore_height,
        address: None,
        network: None,
        state: RestoreState::Unknown,
        warning: None,
        unit_started,
    };
    drop(request); // seed/password/offset cleared before any follow-up RPC
    let address = match response {
        Ok(address) => address,
        Err(error) => {
            restored.warning = Some(format!("Restoration was not confirmed: {error} Check the wallet list before attempting another restoration. A timeout does not undo a wallet creation."));
            return Ok(restored);
        }
    };
    let parsed = match address::parse(&address) {
        Ok(address) => address,
        Err(_) => {
            restored.warning = Some("Monero answered restoration but its public address was invalid. Inspect the wallet list; do not repeat restoration blindly.".into());
            return Ok(restored);
        }
    };
    restored.address = Some(parsed.text);
    restored.network = Some(parsed.net);
    restored.state = RestoreState::NeedsSetup;
    if parsed.net != ctx.settings.node.network.net_type() {
        restored.warning = Some("The restored wallet reports a different network. Choose its network before opening it.".into());
        return Ok(restored);
    }
    if cancel.is_cancelled() {
        restored.warning = Some("The wallet was restored. Node setup was cancelled; open the restored wallet to continue.".into());
        return Ok(restored);
    }
    progress(RestorePhase::ConfiguringNode);
    let mut fresh = ctx.clone();
    fresh.settings.wallet.account = 0;
    match wallet_session::install_daemon_and_record(&fresh, w, &restored.name) {
        Ok(_) => {
            restored.state = RestoreState::Ready;
            if crate::capabilities::note(w, crate::capabilities::WalletKind::Deterministic).is_err()
            {
                restored.warning = Some(
                    "The restored wallet is ready, but its type hint could not be stored.".into(),
                );
            }
            progress(RestorePhase::Ready);
        }
        Err(error) => {
            restored.warning = Some(format!(
            "The wallet was restored, but setup did not finish: {error} Open it again to continue."
        ))
        }
    }
    Ok(restored)
}
