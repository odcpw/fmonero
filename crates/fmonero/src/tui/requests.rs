//! Payment URIs are parsed and reviewed locally before populating a payment form.
use super::{hit::Button, screens, Modal, Msg, Screen, Tui};
use fmonero_core::payment_uri::PaymentRequest;
use ftui::layout::Rect;
use ftui::widgets::{paragraph::Paragraph, Widget};
use ftui::{Cmd, Frame, KeyCode, KeyEvent};

pub(super) struct Review {
    pub request: PaymentRequest,
    pub uri: String,
    pub for_payment: bool,
    pub scroll: usize,
    pub max_scroll: std::cell::Cell<usize>,
}

impl Tui {
    pub(super) fn on_request_key(&mut self, mut review: Review, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Escape => return Cmd::none(),
            KeyCode::Down | KeyCode::PageDown => {
                review.scroll = review.scroll.saturating_add(1).min(review.max_scroll.get())
            }
            KeyCode::Up | KeyCode::PageUp => review.scroll = review.scroll.saturating_sub(1),
            KeyCode::Enter if review.for_payment => {
                self.send.to.set_value(review.request.address);
                self.send.amount.set_value(
                    review
                        .request
                        .amount
                        .map_or_else(String::new, |p| p.to_xmr_min(12)),
                );
                self.send
                    .note
                    .set_value(review.request.description.unwrap_or_default());
                self.send.back_to_form();
                self.send.resume();
                return self.set_screen(Screen::Send);
            }
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Enter => self.copy(&review.uri),
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                let address = review.uri.clone();
                self.modal_back = Some(Box::new(Modal::PaymentRequest(review)));
                self.modal = Some(Modal::Qr { address });
                return Cmd::none();
            }
            _ => {}
        }
        self.modal = Some(Modal::PaymentRequest(review));
        Cmd::none()
    }
}

pub(super) fn view(m: &Tui, review: &Review, frame: &mut Frame) {
    let Some(inner) = screens::overlay(
        m,
        frame,
        82,
        frame.bounds().height.saturating_sub(1),
        if review.for_payment {
            " Review payment request "
        } else {
            " Receive request "
        },
    ) else {
        return;
    };
    m.hits.modal(inner);
    let text = if m.privacy() {
        "Payment request validated. Recipient, amount and message are hidden by shared privacy. Nothing has been prepared or sent.".to_string()
    } else {
        format!("Recipient\n{}\nAmount: {}\nName: {}\nDescription: {}\n{}\nNothing has been prepared or sent.", review.request.address,
            review.request.amount.map_or_else(|| "open amount".into(), |p| format!("{} XMR", p.to_xmr_min(12))),
            review.request.recipient_name.as_deref().unwrap_or("—"), review.request.description.as_deref().unwrap_or("—"), review.uri)
    };
    let lines: Vec<_> = text
        .lines()
        .flat_map(|line| screens::wrapped(line, inner.width as usize))
        .collect();
    let room = inner.height.saturating_sub(2) as usize;
    review.max_scroll.set(lines.len().saturating_sub(room));
    let start = review.scroll.min(review.max_scroll.get());
    for (i, line) in lines.iter().skip(start).take(room).enumerate() {
        Paragraph::new(line.as_str()).style(m.theme.text()).render(
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
            frame,
        );
    }
    let row = Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1);
    screens::chip_row(
        m,
        row,
        &[
            (Button::RequestUp, "↑", "Previous"),
            (Button::RequestDown, "↓", "More"),
            (Button::RequestQr, "Q", "QR"),
        ],
        frame,
    );
    screens::modal_actions(
        m,
        inner,
        if review.for_payment {
            "Use request"
        } else {
            "Copy URI"
        },
        frame,
    );
}
