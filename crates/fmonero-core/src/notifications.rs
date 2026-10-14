//! At-most-once incoming candidates. Claim is durable before any caller may deliver.
use crate::{
    history::{self, HistoryQuery, Transfer, TransferKind},
    wallet_ops::{self, WalletIdentity},
    Context, Error, Piconero, WalletClient,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read, os::unix::fs::OpenOptionsExt};
const MAX_SEEN: usize = 100_000;
#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub body: String,
    pub amount: Option<Piconero>,
}
#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    initialized: bool,
    baseline_at: i64,
    seen: BTreeSet<String>,
}

/// Call only for an explicit opt-in. Disabled/closed returns no candidates and
/// does no wallet RPC. Delivery failures must be reported, never requeued.
pub fn claim(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: u32,
    enabled: bool,
) -> Result<Vec<Candidate>, Error> {
    if !enabled || !crate::units::state(ctx, &ctx.settings.wallet.unit)?.active {
        return Ok(vec![]);
    }
    let _lock = crate::send::take_send_lock(ctx)?;
    // app stop shares this lock; recheck after acquiring it before any RPC.
    if !crate::units::state(ctx, &ctx.settings.wallet.unit)?.active {
        return Ok(vec![]);
    }
    wallet_ops::verify(ctx, w, wallet)?;
    let scope = format!("{}:{}:{}", wallet.network, wallet.primary_address, account);
    let path = ctx.paths.state_dir.join(format!(
        "incoming-{}.json",
        crate::artifacts::hex(&Sha256::digest(scope.as_bytes()))
    ));
    let mut ledger = read_ledger(&path)?;
    let mut query = HistoryQuery {
        account_index: account,
        limit: usize::MAX,
        kinds: vec![TransferKind::In, TransferKind::Block, TransferKind::Pool],
        ..Default::default()
    };
    let rows = history::query(ctx, Some(w), &mut query)?;
    // An incomplete pool snapshot is not a trustworthy first baseline.
    if query.pool_unavailable {
        return Err(Error::backend("Incoming payments could not be fully checked. No notification candidates were claimed."));
    }
    let candidates = advance(
        &mut ledger,
        &rows,
        crate::now_unix(),
        ctx.settings.tui.privacy,
        &scope,
    )?;
    persist_ledger(&path, &ledger)?;
    // A crash after this point intentionally loses delivery rather than repeating it.
    Ok(candidates)
}
fn read_ledger(path: &std::path::Path) -> Result<Ledger, Error> {
    let ledger = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(mut f) => {
            let meta = f
                .metadata()
                .map_err(|e| Error::io("inspect", "the notification ledger", path, &e))?;
            if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
                return Err(Error::safety(
                    "The notification ledger is invalid. Notifications were not claimed.",
                ));
            }
            let mut data = Vec::new();
            f.by_ref()
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut data)
                .map_err(|e| Error::io("read", "the notification ledger", path, &e))?;
            serde_json::from_slice::<Ledger>(&data).map_err(|_| {
                Error::safety(
                    "The notification ledger is unreadable. No notifications will be repeated.",
                )
            })?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(e) => return Err(Error::io("read", "the notification ledger", path, &e)),
    };
    if ledger.seen.len() > MAX_SEEN
        || ledger
            .seen
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(Error::safety(
            "The notification ledger is invalid. No notifications were claimed.",
        ));
    }
    Ok(ledger)
}
fn persist_ledger(path: &std::path::Path, ledger: &Ledger) -> Result<(), Error> {
    let bytes = serde_json::to_vec(ledger).map_err(Error::from)?;
    crate::write_private(path, &bytes)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::failure("The notification ledger has no parent."))?;
    std::fs::File::open(parent)
        .and_then(|d| d.sync_all())
        .map_err(|e| Error::io("sync", "the notification ledger directory", parent, &e))
}
fn advance(
    ledger: &mut Ledger,
    rows: &[Transfer],
    now: i64,
    privacy: bool,
    scope: &str,
) -> Result<Vec<Candidate>, Error> {
    let baseline = !ledger.initialized;
    let mut candidates = vec![];
    if baseline {
        ledger.baseline_at = now;
        ledger.initialized = true;
    }
    for row in rows.iter().filter(|r| {
        matches!(
            r.kind,
            TransferKind::In | TransferKind::Block | TransferKind::Pool
        ) && !r.double_spend_seen
    }) {
        let id = crate::artifacts::hex(&Sha256::digest(
            format!(
                "{scope}:{}:{}:{}",
                row.txid, row.subaddr_index.major, row.subaddr_index.minor
            )
            .as_bytes(),
        ));
        if ledger.seen.contains(&id) {
            continue;
        }
        if ledger.seen.len() >= MAX_SEEN {
            return Err(Error::safety("The incoming-notification ledger reached its capacity. No candidates were delivered."));
        }
        ledger.seen.insert(id.clone());
        if !baseline && row.timestamp >= ledger.baseline_at {
            let amount = if privacy { None } else { Some(row.amount) };
            let body = amount.map_or_else(
                || "A payment was observed. Open the wallet to review it.".into(),
                |a| {
                    format!(
                        "{} XMR observed. Open the wallet for confirmation and spendability.",
                        a.to_xmr_string()
                    )
                },
            );
            candidates.push(Candidate {
                id,
                title: "Incoming Monero payment".into(),
                body,
                amount,
            });
        }
    }
    Ok(candidates)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persisted_claim_survives_delivery_failure_and_corruption_refuses_replay() {
        let root = std::env::temp_dir().join(format!(
            "fmonero-notify-{}-{}",
            std::process::id(),
            crate::rpc::random_hex(8).unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("ledger.json");
        let mut ledger = Ledger::default();
        advance(&mut ledger, &[], 100, true, "scope").unwrap();
        persist_ledger(&path, &ledger).unwrap();
        let row = Transfer {
            txid: "new".into(),
            timestamp: 101,
            amount: Piconero(1),
            ..Default::default()
        };
        let mut ledger = read_ledger(&path).unwrap();
        let candidates =
            advance(&mut ledger, std::slice::from_ref(&row), 102, true, "scope").unwrap();
        persist_ledger(&path, &ledger).unwrap();
        assert_eq!(candidates.len(), 1);
        // Simulate delivery failing or the caller dying after durable claim.
        drop(candidates);
        let mut restarted = read_ledger(&path).unwrap();
        assert!(advance(&mut restarted, &[row], 103, true, "scope")
            .unwrap()
            .is_empty());
        std::fs::write(&path, b"broken").unwrap();
        assert!(read_ledger(&path).is_err());
        let missing = root.join("missing").join("ledger.json");
        assert!(persist_ledger(&missing, &ledger).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn historical_baseline_claim_and_privacy_are_at_most_once() {
        let mut ledger = Ledger::default();
        let old = Transfer {
            txid: "old".into(),
            timestamp: 10,
            ..Default::default()
        };
        assert!(
            advance(&mut ledger, std::slice::from_ref(&old), 100, false, "scope")
                .unwrap()
                .is_empty()
        );
        let new = Transfer {
            txid: "new".into(),
            timestamp: 101,
            amount: Piconero(10),
            ..old.clone()
        };
        let found = advance(&mut ledger, std::slice::from_ref(&new), 102, true, "scope").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].amount, None);
        assert!(!found[0].body.contains("XMR"));
        assert!(advance(&mut ledger, &[new, old], 103, false, "scope")
            .unwrap()
            .is_empty());
        let late_history = Transfer {
            txid: "late".into(),
            timestamp: 9,
            ..Default::default()
        };
        assert!(advance(&mut ledger, &[late_history], 104, false, "scope")
            .unwrap()
            .is_empty());
    }
}
