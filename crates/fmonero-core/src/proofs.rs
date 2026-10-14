//! Typed message signing and Monero proofs. No secret-key export and no broadcasting.
use crate::{
    rpc::jsonrpc::Deadline,
    wallet_ops::{self, WalletIdentity},
    Context, Error, Piconero, WalletClient,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SignatureType {
    Spend,
    View,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Signature {
    pub signature: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MessageVerification {
    pub good: bool,
    pub version: u32,
    pub old: bool,
    pub signature_type: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TxVerification {
    pub good: bool,
    pub received: Piconero,
    pub in_pool: bool,
    pub confirmations: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReserveVerification {
    pub good: bool,
    pub total: Piconero,
    pub spent: Piconero,
}
#[derive(Clone, Copy, Debug)]
pub enum ReserveScope {
    All,
    Account { account: u32, amount: Piconero },
}
fn text(s: &str) -> Result<(), Error> {
    if s.len() > 64 * 1024 || s.contains('\0') {
        Err(Error::usage(
            "Message or proof exceeds its input limit or contains a zero byte.",
        ))
    } else {
        Ok(())
    }
}
fn txid(s: &str) -> Result<(), Error> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        Err(Error::usage(
            "Enter a 64-character hexadecimal transaction ID.",
        ))
    } else {
        Ok(())
    }
}
fn address(s: &str) -> Result<(), Error> {
    crate::address::parse(s).map(|_| ())
}

pub fn sign_message(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: u32,
    index: u32,
    kind: SignatureType,
    data: &str,
) -> Result<Signature, Error> {
    text(data)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params<'a> {
        data: &'a str,
        account_index: u32,
        address_index: u32,
        signature_type: SignatureType,
    }
    w.protected(
        "sign",
        &Params {
            data,
            account_index: account,
            address_index: index,
            signature_type: kind,
        },
        Deadline::Transfer,
    )
}
pub fn verify_message(
    w: &WalletClient,
    address_text: &str,
    data: &str,
    signature: &str,
) -> Result<MessageVerification, Error> {
    text(data)?;
    text(signature)?;
    address(address_text)?;
    #[derive(Serialize)]
    struct Params<'a> {
        data: &'a str,
        address: &'a str,
        signature: &'a str,
    }
    w.protected(
        "verify",
        &Params {
            data,
            address: address_text,
            signature,
        },
        Deadline::Probe,
    )
}
pub fn transaction_proof(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    transaction: &str,
    destination: &str,
    message: &str,
) -> Result<Signature, Error> {
    txid(transaction)?;
    address(destination)?;
    text(message)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params<'a> {
        txid: &'a str,
        address: &'a str,
        message: &'a str,
    }
    w.protected(
        "get_tx_proof",
        &Params {
            txid: transaction,
            address: destination,
            message,
        },
        Deadline::Transfer,
    )
}
pub fn check_transaction_proof(
    w: &WalletClient,
    transaction: &str,
    destination: &str,
    message: &str,
    signature: &str,
) -> Result<TxVerification, Error> {
    txid(transaction)?;
    address(destination)?;
    text(message)?;
    text(signature)?;
    #[derive(Serialize)]
    struct Params<'a> {
        txid: &'a str,
        address: &'a str,
        message: &'a str,
        signature: &'a str,
    }
    w.protected(
        "check_tx_proof",
        &Params {
            txid: transaction,
            address: destination,
            message,
            signature,
        },
        Deadline::Transfer,
    )
}
pub fn spend_proof(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    transaction: &str,
    message: &str,
) -> Result<Signature, Error> {
    txid(transaction)?;
    text(message)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params<'a> {
        txid: &'a str,
        message: &'a str,
    }
    w.protected(
        "get_spend_proof",
        &Params {
            txid: transaction,
            message,
        },
        Deadline::Transfer,
    )
}
pub fn check_spend_proof(
    w: &WalletClient,
    transaction: &str,
    message: &str,
    signature: &str,
) -> Result<bool, Error> {
    txid(transaction)?;
    text(message)?;
    text(signature)?;
    #[derive(Serialize)]
    struct Params<'a> {
        txid: &'a str,
        message: &'a str,
        signature: &'a str,
    }
    #[derive(Deserialize)]
    struct Reply {
        good: bool,
    }
    let reply: Reply = w.protected(
        "check_spend_proof",
        &Params {
            txid: transaction,
            message,
            signature,
        },
        Deadline::Transfer,
    )?;
    Ok(reply.good)
}
pub fn reserve_proof(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    scope: ReserveScope,
    message: &str,
) -> Result<Signature, Error> {
    text(message)?;
    let (all, account_index, amount) = match scope {
        ReserveScope::All => (true, 0, Piconero(0)),
        ReserveScope::Account { account, amount } => {
            if amount.0 == 0 {
                return Err(Error::usage(
                    "Reserve proof amount must be greater than zero.",
                ));
            }
            (false, account, amount)
        }
    };
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params<'a> {
        all: bool,
        account_index: u32,
        amount: Piconero,
        message: &'a str,
    }
    w.protected(
        "get_reserve_proof",
        &Params {
            all,
            account_index,
            amount,
            message,
        },
        Deadline::Transfer,
    )
}
pub fn check_reserve_proof(
    w: &WalletClient,
    address_text: &str,
    message: &str,
    signature: &str,
) -> Result<ReserveVerification, Error> {
    address(address_text)?;
    text(message)?;
    text(signature)?;
    #[derive(Serialize)]
    struct Params<'a> {
        address: &'a str,
        message: &'a str,
        signature: &'a str,
    }
    w.protected(
        "check_reserve_proof",
        &Params {
            address: address_text,
            message,
            signature,
        },
        Deadline::Transfer,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proof_inputs_are_bounded_and_transaction_ids_are_exact() {
        assert!(txid(&"a".repeat(64)).is_ok());
        for id in ["a".repeat(63), "g".repeat(64), "a".repeat(65)] {
            assert!(txid(&id).is_err());
        }
        assert!(text(&"a".repeat(65537)).is_err());
        assert!(text("\0").is_err());
    }
}
