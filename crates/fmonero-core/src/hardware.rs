//! Actual Monero device wallet creation. Physical device results are never simulated.
use crate::{
    native_process::{self, NativeProcess},
    restore::{RestoreState, Restored},
    CancelToken, Context, Error, SecretString, WalletClient,
};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Ledger,
    Trezor,
}
pub struct CreateHardware {
    pub name: String,
    pub password: SecretString,
    pub device: Device,
    pub restore_height: u64,
}
/// The device itself confirms any required actions. Password stays in sealed memory.
/// After creation, ordinary open_secret and send::build/build_sweep use Monero's
/// actual device implementation and surface real locked/disconnected refusals.
pub fn create(
    ctx: &Context,
    w: &WalletClient,
    request: CreateHardware,
    cancel: &CancelToken,
) -> Result<Restored, Error> {
    crate::wallet_dir::require_new(ctx, &request.name)?;
    if request.password.is_empty() {
        return Err(Error::usage(
            "Choose a nonempty password for the device wallet file.",
        ));
    }
    let _lock = crate::send::take_send_lock(ctx)?;
    crate::send::refuse_if_send_in_flight(ctx)?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let unit_started = crate::units::ensure_running(
        ctx,
        &ctx.settings.wallet.unit,
        &|| w.get_version().is_ok(),
        ctx.timeouts.unit_probe,
        cancel,
    )?;
    // Monero releases its own wallet lock; fmonero never reads/modifies wallet bytes.
    match w.close_wallet() {
        Ok(()) => {}
        Err(e) if e.exit_code() == 9 => {}
        Err(e) => return Err(e),
    }
    crate::state::clear_last_open(ctx)?;
    crate::wallet_dir::require_new(ctx, &request.name)?;
    let stage = crate::artifacts::PrivateDirectory::new(&ctx.paths.run_dir)?;
    let memory = native_process::sealed_input(request.password.expose_secret().as_bytes())?;
    let network = ctx.settings.node.network.net_type();
    let mut args = native_process::base_args(network);
    args.extend([
        "--offline".into(),
        "--generate-from-device".into(),
        ctx.settings
            .wallet
            .dir
            .join(&request.name)
            .to_string_lossy()
            .into_owned(),
        "--hw-device".into(),
        match request.device {
            Device::Ledger => "Ledger".into(),
            Device::Trezor => "Trezor".into(),
        },
        "--restore-height".into(),
        request.restore_height.to_string(),
        "--password-file".into(),
        "/proc/self/fd/3".into(),
        "--shared-ringdb-dir".into(),
        "ringdb".into(),
        "--command".into(),
        "address".into(),
    ]);
    let mut result = Restored {
        name: request.name.clone(),
        restore_height: request.restore_height,
        address: None,
        network: Some(network),
        state: RestoreState::Unknown,
        warning: None,
        unit_started,
    };
    let native = (|| -> Result<String, Error> {
        let mut process =
            NativeProcess::start(&args, &stage.path(), Some(&memory), ctx.timeouts.open)?;
        process.wait(cancel)?;
        if !process.success() {
            return Err(Error::backend("Monero could not create the hardware wallet. Connect and unlock the selected device and open its Monero application."));
        }
        process
            .public_addresses()
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::backend("Monero did not confirm a generated hardware wallet address.")
            })
    })();
    let address = match native {
        Ok(a) => a,
        Err(e) => {
            result.warning=Some(format!("Hardware creation was not confirmed: {e} Inspect the wallet list before trying again."));
            return Ok(result);
        }
    };
    result.address = Some(address.clone());
    result.state = RestoreState::NeedsSetup;
    if cancel.is_cancelled() {
        result.warning = Some("The device wallet was created. Open it to continue.".into());
        return Ok(result);
    }
    let setup = (|| -> Result<(), Error> {
        w.open_wallet(&request.name, request.password.expose_secret())?;
        if w.get_address(0)?.primary != address {
            return Err(Error::safety(
                "The device wallet returned an unexpected address on reopen.",
            ));
        }
        let mut fresh = ctx.clone();
        fresh.settings.wallet.account = 0;
        crate::wallet_session::install_daemon_and_record(&fresh, w, &request.name)?;
        crate::capabilities::note(
            w,
            match request.device {
                Device::Ledger => crate::capabilities::WalletKind::Ledger,
                Device::Trezor => crate::capabilities::WalletKind::Trezor,
            },
        )?;
        Ok(())
    })();
    match setup {
        Ok(()) => result.state = RestoreState::Ready,
        Err(e) => {
            result.warning = Some(format!(
                "The device wallet exists, but opening/setup did not finish: {e}"
            ))
        }
    };
    Ok(result)
}
