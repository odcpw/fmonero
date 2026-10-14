use clap::Subcommand;
use fmonero_core::{
    app, notifications, proc, settings, state, wallet_ops, CancelToken, Context, Error,
    WalletClient,
};
use std::time::Duration;

#[derive(Subcommand)]
pub enum NotificationCmd {
    /// Enable generic desktop alerts for new incoming observations
    Enable,
    /// Stop incoming desktop alerts
    Disable,
    /// Show the local opt-in without contacting any backend
    Status,
    /// Claim new observations and request one generic desktop alert for the batch
    Poll,
}

pub fn run(
    ctx: &Context,
    cmd: NotificationCmd,
    cancel: &CancelToken,
) -> Result<serde_json::Value, Error> {
    match cmd {
        NotificationCmd::Enable | NotificationCmd::Disable => {
            let enabled = matches!(cmd, NotificationCmd::Enable);
            let mut config = settings::Settings::load(&ctx.paths)?;
            config.set("tui.notifications", if enabled { "true" } else { "false" })?;
            config.save(ctx)?;
            Ok(
                serde_json::json!({"enabled": enabled, "content": "generic", "delivery": "at_most_once"}),
            )
        }
        NotificationCmd::Status => Ok(serde_json::json!({
            "enabled": ctx.settings.tui.notifications, "content": "generic", "delivery": "at_most_once",
        })),
        NotificationCmd::Poll => poll(ctx, cancel),
    }
}

/// Safe for a UI worker. Disabled or closed returns before constructing a wallet client.
pub fn poll(ctx: &Context, cancel: &CancelToken) -> Result<serde_json::Value, Error> {
    let inactive = || {
        serde_json::json!({"enabled": ctx.settings.tui.notifications,
        "claimed": 0, "desktop_requests_accepted": 0, "delivery": "at_most_once"})
    };
    if !ctx.settings.tui.notifications
        || !app::status(ctx)?.wallet_running
        || state::last_open(ctx)?.is_none()
    {
        return Ok(inactive());
    }
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    if proc::which("notify-send").is_none() {
        return Err(Error::backend(
            "Desktop notifications need notify-send. No observations were claimed.",
        ));
    }
    let wallet = WalletClient::from_ctx(ctx)?;
    let identity = wallet_ops::identity(ctx, &wallet)?;
    let candidates =
        notifications::claim(ctx, &wallet, &identity, ctx.settings.wallet.account, true)?;
    if candidates.is_empty() {
        return Ok(inactive());
    }
    // Generic content is deliberate even while privacy is off: a later privacy
    // toggle cannot retract a notification retained by the desktop daemon.
    let body = if candidates.len() == 1 {
        "An incoming payment was observed. Open fmonero to review its confirmation and availability.".to_owned()
    } else {
        format!("{} incoming payment observations. Open fmonero to review confirmation and availability.", candidates.len())
    };
    let output = proc::run_with_deadline(
        &[
            "notify-send",
            "--app-name=fmonero",
            "--",
            "Incoming Monero payment",
            &body,
        ],
        Duration::from_secs(3),
        Some(cancel),
    )?;
    if !output.success() {
        return Err(Error::failure("The desktop did not accept the incoming-payment alert. These observations were claimed and will not be automatically repeated."));
    }
    Ok(
        serde_json::json!({"enabled": true, "claimed": candidates.len(),
        "desktop_requests_accepted": 1, "delivery": "at_most_once", "content": "generic"}),
    )
}
