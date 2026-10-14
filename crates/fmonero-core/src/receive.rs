//! Receiving identity, labeled subaddresses and evidence from actual wallet observations.
use crate::{
    history::{self, HistoryQuery, Transfer, TransferKind},
    wallet_ops::{self, WalletIdentity},
    Context, Error, Piconero, WalletClient,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct ReceivingAddress {
    pub wallet: WalletIdentity,
    pub account_index: u32,
    pub address_index: u32,
    pub address: String,
    pub label: String,
}
/// Read the account's complete address set from the captured wallet, excluding
/// concurrent wallet replacement. This read helper owns its lock.
pub fn addresses(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: u32,
) -> Result<crate::rpc::wallet::AddressSet, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    wallet_ops::verify(ctx, w, wallet)?;
    w.get_address(account)
}

pub fn new_address(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: u32,
    label: &str,
) -> Result<ReceivingAddress, Error> {
    wallet_ops::text(label)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    // Check the account before dispatch, and never retry a timed-out creation.
    w.get_address(account)?;
    let new = w.create_address(account, label)?;
    if crate::address::parse(&new.address)?.net != wallet.network {
        return Err(Error::upstream(
            "The wallet returned an address on another network.",
        ));
    }
    Ok(ReceivingAddress {
        wallet: wallet.clone(),
        account_index: account,
        address_index: new.address_index,
        address: new.address,
        label: label.into(),
    })
}
pub fn select(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: u32,
    index: u32,
) -> Result<ReceivingAddress, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    wallet_ops::verify(ctx, w, wallet)?;
    let row = w
        .get_address(account)?
        .addresses
        .into_iter()
        .find(|a| a.address_index == index)
        .ok_or_else(wallet_ops::stale)?;
    Ok(ReceivingAddress {
        wallet: wallet.clone(),
        account_index: account,
        address_index: index,
        address: row.address,
        label: row.label,
    })
}
/// Rename only the exact receiving row the user reviewed. This function owns
/// the payment lock; callers must not hold another send lock around it.
pub fn rename(
    ctx: &Context,
    w: &WalletClient,
    expected: &ReceivingAddress,
    label: &str,
) -> Result<(), Error> {
    wallet_ops::text(label)?;
    let _lock = wallet_ops::guard(ctx, w, &expected.wallet)?;
    let current = w.get_address(expected.account_index)?;
    if !current.addresses.iter().any(|row| {
        row.address_index == expected.address_index
            && row.address == expected.address
            && row.label == expected.label
    }) {
        return Err(wallet_ops::stale());
    }
    w.label_address(expected.account_index, expected.address_index, label)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiveStage {
    Unseen,
    Receiving,
    Confirmed,
    Spendable,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReceiptObservation {
    pub txid: String,
    pub amount: Piconero,
    pub stage: ReceiveStage,
    pub confirmations: Option<u64>,
    pub double_spend_seen: bool,
    pub unspent: Option<Piconero>,
    pub unlocked_unspent: Option<Piconero>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub address: ReceivingAddress,
    pub stage: ReceiveStage,
    pub received: Piconero,
    pub confirmed: Piconero,
    pub spendable: Piconero,
    pub receipts: Vec<ReceiptObservation>,
    pub wallet_height: Option<u64>,
    pub pool_unavailable: bool,
    pub spend_authority: Option<bool>,
    pub spent_status_known: bool,
}
/// Requests live history; never estimates confirmation against the node's height.
pub fn observe(
    ctx: &Context,
    w: &WalletClient,
    address: &ReceivingAddress,
) -> Result<Observation, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    wallet_ops::verify(ctx, w, &address.wallet)?;
    let current = w.get_address(address.account_index)?;
    if !current
        .addresses
        .iter()
        .any(|a| a.address_index == address.address_index && a.address == address.address)
    {
        return Err(wallet_ops::stale());
    }
    let mut query = HistoryQuery {
        account_index: address.account_index,
        limit: usize::MAX,
        kinds: vec![TransferKind::In, TransferKind::Block, TransferKind::Pool],
        ..Default::default()
    };
    let rows = history::query(ctx, Some(w), &mut query)?;
    #[derive(serde::Deserialize)]
    struct Output {
        amount: Piconero,
        spent: bool,
        tx_hash: String,
        subaddr_index: crate::SubaddrIndex,
        key_image: String,
        frozen: bool,
        unlocked: bool,
    }
    #[derive(serde::Deserialize)]
    struct Reply {
        #[serde(default)]
        transfers: Vec<Output>,
    }
    let reply:Reply=w.protected("incoming_transfers",&serde_json::json!({"transfer_type":"all","account_index":address.account_index,"subaddr_indices":[address.address_index]}),crate::rpc::jsonrpc::Deadline::Probe)?;
    let mut evidence = std::collections::BTreeMap::<String, OutputEvidence>::new();
    for output in reply.transfers.into_iter().filter(|o| {
        o.subaddr_index.major == address.account_index
            && o.subaddr_index.minor == address.address_index
    }) {
        let item = evidence.entry(output.tx_hash).or_insert(OutputEvidence {
            known: true,
            unspent: Piconero(0),
            unlocked: Piconero(0),
        });
        item.known &= !output.key_image.is_empty();
        if !output.spent {
            item.unspent = item.unspent.checked_add(output.amount)?;
            if output.unlocked && !output.frozen {
                item.unlocked = item.unlocked.checked_add(output.amount)?;
            }
        }
    }
    summarize(
        address.clone(),
        &rows,
        query.wallet_height,
        query.adjusted_now,
        query.pool_unavailable,
        &evidence,
    )
}
struct OutputEvidence {
    known: bool,
    unspent: Piconero,
    unlocked: Piconero,
}
fn summarize(
    address: ReceivingAddress,
    rows: &[Transfer],
    height: Option<u64>,
    now: Option<i64>,
    pool_unavailable: bool,
    evidence: &std::collections::BTreeMap<String, OutputEvidence>,
) -> Result<Observation, Error> {
    let mut out = Observation {
        address,
        stage: if pool_unavailable {
            ReceiveStage::Unknown
        } else {
            ReceiveStage::Unseen
        },
        received: Piconero(0),
        confirmed: Piconero(0),
        spendable: Piconero(0),
        receipts: vec![],
        wallet_height: height,
        pool_unavailable,
        spend_authority: None,
        spent_status_known: true,
    };
    for row in rows.iter().filter(|t| {
        matches!(
            t.kind,
            TransferKind::In | TransferKind::Block | TransferKind::Pool
        ) && t.subaddr_index.major == out.address.account_index
            && t.subaddr_index.minor == out.address.address_index
    }) {
        let confirmations = history::confirmations(row, height);
        let outputs = evidence.get(&row.txid).filter(|o| o.known);
        let unspent = outputs.map(|o| o.unspent);
        let unlocked_unspent = outputs.map(|o| o.unlocked);
        if outputs.is_none() && row.kind != TransferKind::Pool {
            out.spent_status_known = false;
        }
        let stage = if row.double_spend_seen || height.is_none() {
            ReceiveStage::Unknown
        } else if row.kind == TransferKind::Pool || confirmations == Some(0) {
            ReceiveStage::Receiving
        } else if outputs.is_some_and(|o| o.unlocked.0 > 0) {
            ReceiveStage::Spendable
        } else if history::locked(row, height, now) == Some(false) && outputs.is_none() {
            ReceiveStage::Unknown
        } else {
            ReceiveStage::Confirmed
        };
        let add = |total: &mut Piconero| -> Result<(), Error> {
            total.0 = total
                .0
                .checked_add(row.amount.0)
                .ok_or_else(|| Error::upstream("Received amount exceeds the supported range."))?;
            Ok(())
        };
        if !row.double_spend_seen {
            add(&mut out.received)?;
        }
        if !row.double_spend_seen && confirmations.is_some_and(|n| n > 0) {
            add(&mut out.confirmed)?;
        }
        if stage == ReceiveStage::Spendable {
            out.spendable = out
                .spendable
                .checked_add(unlocked_unspent.unwrap_or_default())?;
        }
        out.receipts.push(ReceiptObservation {
            txid: row.txid.clone(),
            amount: row.amount,
            stage,
            confirmations,
            double_spend_seen: row.double_spend_seen,
            unspent,
            unlocked_unspent,
        });
    }
    if !out.receipts.is_empty() {
        out.stage = if out
            .receipts
            .iter()
            .any(|r| r.stage == ReceiveStage::Unknown)
        {
            ReceiveStage::Unknown
        } else if out
            .receipts
            .iter()
            .any(|r| r.stage == ReceiveStage::Receiving)
        {
            ReceiveStage::Receiving
        } else if out
            .receipts
            .iter()
            .all(|r| r.stage == ReceiveStage::Spendable)
        {
            ReceiveStage::Spendable
        } else {
            ReceiveStage::Confirmed
        };
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_evidence_tracks_pool_confirmation_unlock_and_unknown() {
        let address = ReceivingAddress {
            wallet: WalletIdentity {
                name: "scratch".into(),
                network: crate::NetType::Mainnet,
                primary_address: String::new(),
            },
            account_index: 0,
            address_index: 1,
            address: String::new(),
            label: String::new(),
        };
        let mut evidence = std::collections::BTreeMap::new();
        evidence.insert(
            String::new(),
            OutputEvidence {
                known: true,
                unspent: Piconero(13),
                unlocked: Piconero(0),
            },
        );
        let mut t = Transfer {
            amount: Piconero(13),
            subaddr_index: crate::SubaddrIndex { major: 0, minor: 1 },
            kind: TransferKind::Pool,
            ..Default::default()
        };
        assert_eq!(
            summarize(
                address.clone(),
                &[t.clone()],
                Some(10),
                Some(0),
                false,
                &evidence
            )
            .unwrap()
            .stage,
            ReceiveStage::Receiving
        );
        t.kind = TransferKind::In;
        t.height = 10;
        assert_eq!(
            summarize(
                address.clone(),
                &[t.clone()],
                Some(11),
                Some(0),
                false,
                &evidence
            )
            .unwrap()
            .stage,
            ReceiveStage::Confirmed
        );
        evidence.get_mut("").unwrap().unlocked = Piconero(13);
        assert_eq!(
            summarize(
                address.clone(),
                &[t.clone()],
                Some(20),
                Some(0),
                false,
                &evidence
            )
            .unwrap()
            .spendable,
            Piconero(13)
        );
        assert_eq!(
            summarize(address.clone(), &[t.clone()], None, None, false, &evidence)
                .unwrap()
                .stage,
            ReceiveStage::Unknown
        );
        evidence.get_mut("").unwrap().known = false;
        let view = summarize(
            address.clone(),
            &[t.clone()],
            Some(20),
            Some(0),
            false,
            &evidence,
        )
        .unwrap();
        assert_eq!(view.stage, ReceiveStage::Unknown);
        assert_eq!(view.spendable, Piconero(0));
        assert!(!view.spent_status_known);
        assert_eq!(view.spend_authority, None);
        t.double_spend_seen = true;
        assert_eq!(
            summarize(address, &[t], Some(20), Some(0), false, &evidence)
                .unwrap()
                .received,
            Piconero(0)
        );
    }
}
