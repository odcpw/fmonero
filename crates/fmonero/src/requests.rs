use clap::Subcommand;
use fmonero_core::{payment_uri, Context, Error, Piconero, WalletClient};

#[derive(Subcommand)]
pub enum RequestCmd {
    /// Create an exact Monero payment URI; starts no service and sends nothing
    Create {
        /// Use this address, or choose one from the open wallet
        #[arg(long)]
        address: Option<String>,
        #[arg(long)]
        account: Option<u32>,
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long)]
        amount: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        recipient_name: Option<String>,
        /// Draw the complete request as a QR in terminal output
        #[arg(long)]
        qr: bool,
    },
    /// Validate a received request for review; prepares and sends nothing
    Parse { uri: String },
}

pub fn run(ctx: &Context, cmd: RequestCmd) -> Result<serde_json::Value, Error> {
    let network = ctx.settings.node.network.net_type();
    let (request, uri) = match cmd {
        RequestCmd::Parse { uri } => (payment_uri::parse(&uri, network)?, uri),
        RequestCmd::Create {
            address,
            account,
            index,
            amount,
            description,
            recipient_name,
            ..
        } => {
            let address = match address {
                Some(address) => address,
                None => WalletClient::from_ctx(ctx)?
                    .get_address(account.unwrap_or(ctx.settings.wallet.account))?
                    .addresses
                    .into_iter()
                    .find(|row| row.address_index == index)
                    .map(|row| row.address)
                    .ok_or_else(|| Error::usage("This account has no address with that index."))?,
            };
            let request = payment_uri::PaymentRequest {
                address,
                amount: amount.as_deref().map(Piconero::parse).transpose()?,
                description,
                recipient_name,
            };
            let uri = payment_uri::format(&request, network)?;
            (request, uri)
        }
    };
    Ok(serde_json::json!({
        "uri": uri, "address": request.address,
        "amount_pico": request.amount.map(|v| v.0),
        "amount_xmr": request.amount.map(Piconero::to_xmr_string),
        "description": request.description, "recipient_name": request.recipient_name,
        "network": network, "needs_payment_review": true,
    }))
}
