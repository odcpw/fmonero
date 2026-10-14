use clap::ValueEnum;
use fmonero_core::{
    log_sources::{self, LogQuery, LogSource},
    CancelToken, Context, Error,
};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Source {
    #[default]
    Wallet,
    Node,
    WalletService,
    NodeService,
    Application,
}

pub fn read(
    ctx: &Context,
    source: Source,
    lines: usize,
    search: Option<String>,
    cancel: &CancelToken,
) -> Result<serde_json::Value, Error> {
    let source = match source {
        Source::Wallet => LogSource::Wallet,
        Source::Node => LogSource::Node,
        Source::WalletService => LogSource::WalletService,
        Source::NodeService => LogSource::NodeService,
        Source::Application => LogSource::Application,
    };
    let result = log_sources::read(
        ctx,
        &LogQuery {
            source,
            lines,
            search,
            previous: None,
        },
        cancel,
    )?;
    Ok(serde_json::to_value(result)?)
}
