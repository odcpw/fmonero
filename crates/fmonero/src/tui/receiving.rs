//! Receipt evidence is independent of wallet signing capability.
use super::{advanced::ResultView, hit::Button, screens, Modal, Msg, Tui};
use fmonero_core::{
    receive::{self, Observation, ReceiveStage},
    wallet_ops, Piconero, WalletClient,
};
use ftui::layout::Rect;
use ftui::widgets::{paragraph::Paragraph, Widget};
use ftui::{Cmd, Frame};

#[derive(Default)]
pub(super) struct ReceiveUi {
    pub observed: Option<Observation>,
    pub request: Option<(String, Piconero)>,
    pub following: bool,
    pub inflight: bool,
    pub error: Option<String>,
    pub show_details: bool,
}
fn stage(stage: ReceiveStage) -> &'static str {
    match stage {
        ReceiveStage::Unseen => "No receipt seen",
        ReceiveStage::Receiving => "Receiving · awaiting confirmation",
        ReceiveStage::Confirmed => "Confirmed",
        ReceiveStage::Spendable => "Unlocked outputs observed",
        ReceiveStage::Unknown => "Receipt state needs more evidence",
    }
}
impl Tui {
    pub(super) fn track_receipts(&mut self) -> Cmd<Msg> {
        self.wallet.pane = super::Pane::Subaddresses;
        self.receiving.following = !self.receiving.following;
        if self.receiving.following {
            self.load_receipts()
        } else {
            Cmd::none()
        }
    }
    pub(super) fn load_receipts(&mut self) -> Cmd<Msg> {
        if self.receiving.inflight
            || self.action.is_some()
            || !self.config_valid
            || !self.status.as_ref().is_some_and(|s| s.wallet.open)
        {
            return Cmd::none();
        }
        let Some(row) = self.wallet.addresses.as_ref().and_then(|a| {
            a.addresses
                .get(self.wallet.table.borrow().selected.unwrap_or(0))
        }) else {
            self.receiving.error =
                Some("Select a receiving address after the wallet loads.".into());
            return Cmd::none();
        };
        let index = row.address_index;
        let address_text = row.address.clone();
        let epoch = self.read_epoch;
        let account = self.account_index();
        let ctx = self.ctx.clone();
        self.receiving.inflight = true;
        Cmd::task(move || {
            let read = || -> Result<Observation, fmonero_core::Error> {
                let w = WalletClient::from_ctx(&ctx)?;
                let wallet = wallet_ops::identity(&ctx, &w)?;
                let address = receive::select(&ctx, &w, &wallet, account, index)?;
                if address.address != address_text {
                    return Err(fmonero_core::Error::safety(
                        "The receiving address changed. Select it again.",
                    ));
                }
                receive::observe(&ctx, &w, &address)
            };
            Msg::Received(epoch, Box::new(read().map_err(|e| e.to_string())))
        })
    }
    pub(super) fn receipt_details(&mut self) -> Cmd<Msg> {
        let Some(observation) = self
            .receiving
            .observed
            .as_ref()
            .filter(|o| self.selected_address().as_deref() == Some(o.address.address.as_str()))
        else {
            self.receiving.following = true;
            self.receiving.show_details = true;
            return self.load_receipts();
        };
        let mut lines = vec![
            stage(observation.stage).into(),
            observation.address.address.clone(),
            format!(
                "Received {} XMR\nConfirmed {} XMR\nObserved unlocked/unspent {} XMR",
                observation.received.to_xmr_min(12),
                observation.confirmed.to_xmr_min(12),
                observation.spendable.to_xmr_min(12)
            ),
            if observation.spent_status_known {
                "Spent status has output evidence. Signing authority is not inferred.".into()
            } else {
                "Spent status is unknown. A view-only wallet needs matching key images; no spend authority is implied.".into()
            },
        ];
        if let Some((address, amount)) = &self.receiving.request {
            if address == &observation.address.address {
                lines.push(format!(
                    "Requested {} XMR · {}",
                    amount.to_xmr_min(12),
                    if observation.received.0 >= amount.0 {
                        "received amount met; inspect confirmation evidence"
                    } else {
                        "awaiting remaining amount"
                    }
                ));
            }
        }
        if observation.pool_unavailable {
            lines.push(
                "Pool visibility is unavailable. An unseen payment may still be on its way.".into(),
            );
        }
        for receipt in &observation.receipts {
            lines.push(format!(
                "{}\n{} XMR · {} · confirmations {}{}",
                receipt.txid,
                receipt.amount.to_xmr_min(12),
                stage(receipt.stage),
                receipt
                    .confirmations
                    .map_or_else(|| "unknown".into(), |n| n.to_string()),
                if receipt.double_spend_seen {
                    " · double-spend warning"
                } else {
                    ""
                }
            ));
        }
        let mut result = ResultView::complete("Receiving evidence", lines);
        result.failed = observation.stage == ReceiveStage::Unknown
            || observation.receipts.iter().any(|r| r.double_spend_seen);
        self.modal = Some(Modal::Result(result));
        Cmd::none()
    }
}
pub(super) fn view(m: &Tui, area: Rect, frame: &mut Frame) {
    let message = if m.receiving.inflight {
        "Reading receipt evidence…".into()
    } else if m.privacy() {
        "Receipt amounts and identities hidden by privacy.".into()
    } else if let Some(error) = &m.receiving.error {
        error.clone()
    } else if let Some(o) = &m.receiving.observed {
        if m.selected_address().as_deref() == Some(o.address.address.as_str()) {
            format!(
                "{} · {} XMR received",
                stage(o.stage),
                o.received.to_xmr_min(12)
            )
        } else {
            "Select Track to observe the selected address.".into()
        }
    } else {
        "Track the selected address for receipt, confirmation and unlocked-output evidence.".into()
    };
    Paragraph::new(message)
        .style(if m.receiving.error.is_some() {
            m.theme.warn()
        } else {
            m.theme.text()
        })
        .render(Rect::new(area.x, area.y, area.width, 1), frame);
    if area.height > 1 {
        screens::chip_row(
            m,
            Rect::new(area.x, area.y + 1, area.width, area.height - 1),
            &[
                (
                    Button::TrackReceipts,
                    "T",
                    if m.receiving.following {
                        "Pause"
                    } else {
                        "Track"
                    },
                ),
                (Button::ReceiptDetails, "I", "All info"),
            ],
            frame,
        );
    }
}
