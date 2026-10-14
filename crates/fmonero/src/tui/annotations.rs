//! Annotation forms retain the engine identity and prior value they actually reviewed.
use super::{subs::Outcome, Prompt};
use fmonero_core::{receive, wallet_ops, Context, Error, Transfer, WalletClient};

pub(super) enum Job {
    LabelForm {
        account: u32,
        row: fmonero_core::rpc::wallet::SubaddressRow,
    },
    NoteForm(Transfer),
    Save {
        what: Prompt,
        value: String,
    },
}
impl Job {
    pub fn execute(self, ctx: &Context) -> Result<Outcome, Error> {
        let w = WalletClient::from_ctx(ctx)?;
        match self {
            Self::LabelForm { account, row } => {
                let identity = wallet_ops::identity(ctx, &w)?;
                let address = receive::select(ctx, &w, &identity, account, row.address_index)?;
                if address.address != row.address || address.label != row.label {
                    return Err(Error::safety(
                        "The address changed. Select it again before renaming.",
                    ));
                }
                Ok(Outcome::AnnotationForm {
                    title: format!(" Name address #{} ", address.address_index),
                    field: "Name ".into(),
                    value: address.label.clone(),
                    what: Prompt::Label { address },
                })
            }
            Self::NoteForm(row) => {
                let wallet = wallet_ops::identity(ctx, &w)?;
                let current = w.get_transfer_by_txid(&row.txid, row.subaddr_index.major)?;
                if !current.as_ref().is_some_and(|current| {
                    current.txid == row.txid
                        && current.note == row.note
                        && current.subaddr_index.major == row.subaddr_index.major
                }) {
                    return Err(Error::safety(
                        "The payment note changed. Select the payment again before editing.",
                    ));
                }
                Ok(Outcome::AnnotationForm {
                    title: format!(
                        " Note for {} XMR · {} · {} ",
                        row.amount.to_xmr_min(12),
                        row.kind.words(),
                        super::screens::moment(row.timestamp)
                    ),
                    field: "Note ".into(),
                    value: row.note.clone(),
                    what: Prompt::Note {
                        wallet,
                        txid: row.txid,
                        account: row.subaddr_index.major,
                        expected: row.note,
                    },
                })
            }
            Self::Save {
                what: Prompt::Label { address },
                value,
            } => {
                receive::rename(ctx, &w, &address, &value)?;
                Ok(Outcome::Note("Address label saved.".into()))
            }
            Self::Save {
                what:
                    Prompt::Note {
                        wallet,
                        txid,
                        account,
                        expected,
                    },
                value,
            } => {
                fmonero_core::history::set_note(
                    ctx, &w, &wallet, &txid, account, &expected, &value,
                )?;
                Ok(Outcome::Note("Payment note saved.".into()))
            }
            Self::Save {
                what: Prompt::NewAddress { .. },
                ..
            } => Err(Error::usage(
                "Use the new-address action to create an address.",
            )),
        }
    }
}
