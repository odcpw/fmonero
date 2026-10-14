//! Deterministic exact exports with explicit atomic private destinations.
use crate::{
    artifacts::{self, ArtifactReceipt},
    history::{self, HistoryQuery, Transfer},
    Context, Error, WalletClient,
};
use std::path::Path;
#[derive(Clone, Copy, Debug)]
pub enum ExportFormat {
    Json,
    Csv,
}
pub fn export(
    ctx: &Context,
    w: Option<&WalletClient>,
    query: &mut HistoryQuery,
    format: ExportFormat,
    path: &Path,
) -> Result<ArtifactReceipt, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    let rows = history::query(ctx, w, query)?;
    artifacts::write_new(path, &encode(&rows, format)?)
}
pub fn encode(rows: &[Transfer], format: ExportFormat) -> Result<Vec<u8>, Error> {
    if rows.len() > 100_000 {
        return Err(Error::usage(
            "Export at most 100000 history rows at a time.",
        ));
    }
    let mut rows: Vec<&Transfer> = rows.iter().collect();
    rows.sort_by(|a, b| {
        (
            a.timestamp,
            &a.txid,
            a.kind.as_str(),
            a.subaddr_index.major,
            a.subaddr_index.minor,
            &a.address,
        )
            .cmp(&(
                b.timestamp,
                &b.txid,
                b.kind.as_str(),
                b.subaddr_index.major,
                b.subaddr_index.minor,
                &b.address,
            ))
    });
    let out = match format {
        ExportFormat::Json => {
            // Serialize borrowed rows one at a time into a bounded writer; do not
            // construct an unbounded second JSON tree before checking the limit.
            let mut out = crate::secrets::PrivateBuffer::new(artifacts::MAX_ARTIFACT_BYTES);
            serde_json::to_writer_pretty(&mut out, &ExportRows(&rows)).map_err(|_| {
                Error::usage("The export is too large or could not be encoded. Choose fewer rows.")
            })?;
            use std::io::Write;
            out.write_all(b"\n")
                .map_err(|_| Error::usage("The export is too large. Choose fewer rows."))?;
            out.as_slice().to_vec()
        }
        ExportFormat::Csv => {
            let mut out=String::from("txid,type,height,timestamp,amount_pico,amount_xmr,fee_pico,fee_xmr,account_index,address_index,address,note,unlock_time,double_spend_seen\r\n");
            for t in rows {
                let fields = [
                    cell(&t.txid),
                    cell(t.kind.as_str()),
                    t.height.to_string(),
                    t.timestamp.to_string(),
                    t.amount.0.to_string(),
                    t.amount.to_xmr_string(),
                    t.fee.0.to_string(),
                    t.fee.to_xmr_string(),
                    t.subaddr_index.major.to_string(),
                    t.subaddr_index.minor.to_string(),
                    cell(&t.address),
                    cell(&t.note),
                    t.unlock_time.to_string(),
                    t.double_spend_seen.to_string(),
                ];
                out.push_str(&fields.join(","));
                out.push_str("\r\n");
                if out.len() > artifacts::MAX_ARTIFACT_BYTES {
                    return Err(Error::usage("The export is too large. Choose fewer rows."));
                }
            }
            out.into_bytes()
        }
    };
    if out.len() > artifacts::MAX_ARTIFACT_BYTES {
        return Err(Error::usage("The export is too large. Choose fewer rows."));
    }
    Ok(out)
}
#[derive(serde::Serialize)]
struct ExportRow<'a> {
    txid: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    height: u64,
    timestamp: i64,
    amount_pico: u64,
    amount_xmr: String,
    fee_pico: u64,
    fee_xmr: String,
    account_index: u32,
    address_index: u32,
    address: &'a str,
    note: &'a str,
    unlock_time: u64,
    double_spend_seen: bool,
    destinations: Destinations<'a>,
}
struct ExportRows<'a>(&'a [&'a Transfer]);
impl serde::Serialize for ExportRows<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for t in self.0 {
            seq.serialize_element(&ExportRow {
                txid: &t.txid,
                kind: t.kind.as_str(),
                height: t.height,
                timestamp: t.timestamp,
                amount_pico: t.amount.0,
                amount_xmr: t.amount.to_xmr_string(),
                fee_pico: t.fee.0,
                fee_xmr: t.fee.to_xmr_string(),
                account_index: t.subaddr_index.major,
                address_index: t.subaddr_index.minor,
                address: &t.address,
                note: &t.note,
                unlock_time: t.unlock_time,
                double_spend_seen: t.double_spend_seen,
                destinations: Destinations(t),
            })?;
        }
        seq.end()
    }
}
struct Destinations<'a>(&'a Transfer);
impl serde::Serialize for Destinations<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        #[derive(serde::Serialize)]
        struct Row<'a> {
            address: &'a str,
            amount_pico: u64,
            amount_xmr: String,
        }
        let mut seq = s.serialize_seq(Some(self.0.destinations.len()))?;
        for d in &self.0.destinations {
            seq.serialize_element(&Row {
                address: &d.address,
                amount_pico: d.amount.0,
                amount_xmr: d.amount.to_xmr_string(),
            })?;
        }
        seq.end()
    }
}
fn cell(value: &str) -> String {
    // Spreadsheet formulas are data: prefix dangerous text, then quote CSV syntax.
    let prefix = if value.trim_start().starts_with(['=', '+', '-', '@'])
        || value.starts_with(['\t', '\r', '\n'])
    {
        "'"
    } else {
        ""
    };
    format!("\"{prefix}{}\"", value.replace('"', "\"\""))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Piconero;
    #[test]
    fn export_is_order_independent_exact_and_formula_safe() {
        let a = Transfer {
            txid: "a".into(),
            amount: Piconero(u64::MAX),
            note: "=2+2,\"x\"".into(),
            ..Default::default()
        };
        let b = Transfer {
            txid: "b".into(),
            ..a.clone()
        };
        assert_eq!(
            encode(&[b.clone(), a.clone()], ExportFormat::Json).unwrap(),
            encode(&[a.clone(), b], ExportFormat::Json).unwrap()
        );
        let csv = String::from_utf8(encode(&[a], ExportFormat::Csv).unwrap()).unwrap();
        assert!(csv.contains("18446744073709551615,18446744.073709551615"));
        assert!(csv.contains("\"'=2+2,\"\"x\"\"\""));
    }
}
