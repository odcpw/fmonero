//! Native unsigned artifacts prepared by Monero, with no relay or invented sidecar.
use crate::{
    artifacts::{self, ArtifactReceipt},
    rpc::jsonrpc::Deadline,
    secrets::ProtectedText,
    send::{self, NodeGate, TransferRequest},
    wallet_ops::{self, WalletIdentity},
    CancelToken, Context, Error, Piconero, SendRequest, WalletClient,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Debug, Serialize)]
pub struct PreparedUnsigned {
    pub artifact: ArtifactReceipt,
    pub wallet: WalletIdentity,
    pub destination: String,
    pub account_index: u32,
    pub transactions: usize,
    pub amount: Piconero,
    pub fee: Piconero,
}
/// Preparing requires an active view-only wallet that Monero can produce an
/// unsigned txset for. A spend wallet yielding only signed metadata is refused.
pub fn prepare_unsigned(
    ctx: &Context,
    w: &WalletClient,
    gate: NodeGate<'_>,
    wallet: &WalletIdentity,
    request: &SendRequest,
    path: &Path,
    cancel: &CancelToken,
) -> Result<PreparedUnsigned, Error> {
    if request.amount.0 == 0 {
        return Err(Error::usage("Enter an amount greater than zero."));
    }
    let _lock = send::preflight(ctx, w, &request.dest, cancel)?;
    wallet_ops::verify(ctx, w, wallet)?;
    let dest = send::resolve_dest(w, &request.dest)?;
    send::check_recipient(ctx, w, &dest)?;
    send::require_node_current(ctx, w, gate, cancel)?;
    #[derive(Deserialize)]
    struct Reply {
        unsigned_txset: ProtectedText,
        amount_list: Vec<Piconero>,
        fee_list: Vec<Piconero>,
    }
    let reply: Reply = w.protected(
        "transfer_split",
        &TransferRequest::for_send(request, &dest),
        Deadline::Transfer,
    )?;
    let count = reply.amount_list.len();
    if count == 0 || count > 1024 || reply.fee_list.len() != count {
        return Err(Error::upstream(
            "Monero returned inconsistent unsigned transaction amounts.",
        ));
    }
    let amount = reply
        .amount_list
        .into_iter()
        .try_fold(Piconero(0), Piconero::checked_add)?;
    let fee = reply
        .fee_list
        .into_iter()
        .try_fold(Piconero(0), Piconero::checked_add)?;
    if amount != request.amount {
        return Err(Error::safety(
            "The unsigned transaction amount does not match the requested amount.",
        ));
    }
    if request.max_fee.is_some_and(|max| fee > max) {
        return Err(
            Error::safety("The unsigned transaction fee exceeds the selected maximum.")
                .with_reason("fee_above_max"),
        );
    }
    let raw = artifacts::decode_hex(&reply.unsigned_txset.0)?;
    if !raw.starts_with(b"Monero unsigned tx set") {
        return Err(Error::upstream(
            "Monero did not return a native unsigned transaction artifact.",
        ));
    }
    let artifact = artifacts::write_new(path, &raw)?;
    Ok(PreparedUnsigned {
        artifact,
        wallet: wallet.clone(),
        destination: dest,
        account_index: request.account_index,
        transactions: count,
        amount,
        fee,
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipient {
    pub address: String,
    pub amount: Piconero,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionDescription {
    pub amount_in: Piconero,
    pub amount_out: Piconero,
    pub recipients: Vec<Recipient>,
    pub change_amount: Piconero,
    pub change_address: String,
    pub fee: Piconero,
    pub ring_size: u32,
    pub unlock_time: u64,
    pub payment_id: String,
    /// Monero's describe_transfer omits weight; absence is unknown, not zero.
    #[serde(default)]
    pub weight: Option<u64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct UnsignedDescription {
    pub artifact_sha256: String,
    pub transactions: Vec<TransactionDescription>,
}
/// Public inspection uses the actual describe_transfer RPC, which supports unsigned
/// and multisig txsets only. Native sign/submit use their own held engine review.
pub fn describe_unsigned(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    path: &Path,
) -> Result<UnsignedDescription, Error> {
    let bytes = artifacts::read(ctx, path)?;
    if !bytes.starts_with(b"Monero unsigned tx set") {
        return Err(Error::usage(
            "Choose a native Monero unsigned transaction artifact.",
        ));
    }
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    let hex = zeroize::Zeroizing::new(artifacts::hex(&bytes));
    #[derive(Serialize)]
    struct Params<'a> {
        unsigned_txset: crate::secrets::SecretRef<'a>,
        multisig_txset: &'static str,
    }
    #[derive(Deserialize)]
    struct Reply {
        desc: Vec<TransactionDescription>,
    }
    let reply: Reply = w.protected(
        "describe_transfer",
        &Params {
            unsigned_txset: crate::secrets::SecretRef(&hex),
            multisig_txset: "",
        },
        Deadline::Transfer,
    )?;
    if reply.desc.is_empty() || reply.desc.len() > 1024 {
        return Err(Error::upstream(
            "Monero returned an invalid transaction count.",
        ));
    }
    use sha2::{Digest, Sha256};
    Ok(UnsignedDescription {
        artifact_sha256: artifacts::hex(&Sha256::digest(&bytes)),
        transactions: reply.desc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description() -> serde_json::Value {
        // Field shape asserted by Monero 0.18.5.3 functional_tests/cold_signing.py.
        // Synthetic public amounts/addresses test parsing, not a valid transaction.
        serde_json::json!({
            "amount_in":120, "amount_out":118, "recipients":[{"address":"recipient","amount":100}],
            "change_amount":18, "change_address":"change", "fee":2, "ring_size":16,
            "unlock_time":0, "payment_id":""
        })
    }

    #[test]
    fn native_description_distinguishes_omitted_weight_from_real_zero() {
        let value = description();
        let omitted: TransactionDescription = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(omitted.weight, None);
        assert_eq!(
            serde_json::to_value(&omitted).unwrap()["weight"],
            serde_json::Value::Null
        );
        for weight in [0u64, 1536] {
            let mut value = value.clone();
            value["weight"] = weight.into();
            let present: TransactionDescription = serde_json::from_value(value).unwrap();
            assert_eq!(present.weight, Some(weight));
            assert_eq!(serde_json::to_value(&present).unwrap()["weight"], weight);
        }
    }

    #[test]
    fn native_description_requires_every_monetary_field() {
        for field in ["amount_in", "amount_out", "change_amount", "fee"] {
            let mut value = description();
            value.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<TransactionDescription>(value).is_err(),
                "{field}"
            );
        }
        let mut value = description();
        value["recipients"][0]
            .as_object_mut()
            .unwrap()
            .remove("amount");
        assert!(serde_json::from_value::<TransactionDescription>(value).is_err());
        for weight in [
            serde_json::json!(-1),
            serde_json::json!("0"),
            serde_json::json!({}),
        ] {
            let mut value = description();
            value["weight"] = weight;
            assert!(serde_json::from_value::<TransactionDescription>(value).is_err());
        }
    }
}
