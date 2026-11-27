//! One conservative presentation policy for wallet data, logs, errors and forms.
//! A payment plan can be revealed locally; doing so never changes shared privacy.
use ftui::layout::Rect;
use ftui::widgets::{input::TextInput, paragraph::Paragraph, Widget};
use ftui::{Cmd, Frame};

use super::{
    hit::{Button, HitPane},
    screens, send, Modal, Msg, Screen, Tui,
};

impl Tui {
    pub(super) fn revoke_review_authorization(&mut self) {
        self.send.reset_review();
        self.send.typed.clear();
        if let Some(Modal::Native(review)) = &mut self.modal {
            review.revoke_authorization();
        }
    }
    pub(super) fn policy_current(&self) -> bool {
        self.config_valid && super::file_stamp(&self.ctx.paths.config_file()) == self.config_mtime
    }
    pub(super) fn privacy(&self) -> bool {
        self.ctx.settings.tui.privacy || !self.policy_current()
    }
    pub(super) fn toggle_notifications(&mut self) -> Cmd<Msg> {
        let change = (|| -> Result<bool, fmonero_core::Error> {
            let mut settings = fmonero_core::Settings::load(&self.ctx.paths)?;
            let enabled = !settings.tui.notifications;
            settings.set("tui.notifications", if enabled { "true" } else { "false" })?;
            settings.save(&self.ctx)?;
            self.ctx.settings = settings;
            self.config_mtime = super::file_stamp(&self.ctx.paths.config_file());
            Ok(enabled)
        })();
        match change {
            Ok(enabled) => self.note(
                if enabled {
                    "Desktop payment alerts enabled. Historical payments are ignored."
                } else {
                    "Desktop payment alerts disabled."
                },
                false,
            ),
            Err(error) => self.note(error.to_string(), true),
        }
        Cmd::none()
    }
    pub(super) fn toggle_privacy(&mut self) -> Cmd<Msg> {
        let enabled = !self.ctx.settings.tui.privacy;
        match fmonero_core::settings::set_privacy(&self.ctx, enabled) {
            Ok(()) => {
                self.ctx.settings.tui.privacy = enabled;
                self.config_mtime = super::file_stamp(&self.ctx.paths.config_file());
                self.revoke_review_authorization();
                self.note(
                    if enabled {
                        "Privacy on. Shared with the desktop widget."
                    } else {
                        "Privacy off."
                    },
                    false,
                );
            }
            Err(e) => self.note(e.to_string(), true),
        }
        Cmd::none()
    }
}

fn line(m: &Tui, text: &str, area: Rect, frame: &mut Frame) {
    Paragraph::new(text)
        .wrap(ftui::text::wrap::WrapMode::Word)
        .style(m.theme.text())
        .render(area, frame);
}

fn masked(m: &Tui, input: &TextInput, area: Rect, frame: &mut Frame) {
    let mut safe = TextInput::new()
        .with_value("•".repeat(input.value().chars().count()))
        .with_style(m.theme.text());
    safe.set_focused(input.focused());
    safe.render(area, frame);
}

pub(super) fn toggle(m: &Tui, area: Rect, frame: &mut Frame) {
    let text = if m.privacy() {
        "(V) Privacy on"
    } else {
        "(V) Privacy off"
    };
    Paragraph::new(text)
        .style(if m.privacy() {
            m.theme.warn()
        } else {
            m.theme.dimmed()
        })
        .render(area, frame);
    m.hits.button(Button::Privacy, area);
}

pub(super) fn view(m: &Tui, frame: &mut Frame) {
    let b = frame.bounds();
    screens::tab_strip(m, Rect::new(b.x, b.y, b.width, 1), frame);
    let body = Rect::new(b.x, b.y + 2, b.width, b.height.saturating_sub(4));
    line(
        m,
        "Privacy on · personal data hidden",
        Rect::new(b.x, b.y + 1, b.width, 1),
        frame,
    );
    match m.screen {
        Screen::Tools => super::workflows::tools(m, body, frame),
        Screen::Send => send::view(m, body, frame, screens::tier_of(body)),
        Screen::Receive if m.status.as_ref().is_some_and(|s| s.wallet.open) => {
            screens::receive_screen(m, body, frame)
        }
        Screen::Wallet | Screen::Receive => {
            let open = m.status.as_ref().is_some_and(|s| s.wallet.open);
            line(
                m,
                if open {
                    "Wallet open · balance and addresses hidden"
                } else {
                    "No wallet open"
                },
                body,
                frame,
            );
            let mut buttons = Vec::new();
            if m.has_pending() {
                buttons.push((Button::ClearPending, "P", "Clear that record"));
            }
            buttons.extend([
                (Button::OpenWallet, "O", "Open wallet"),
                (Button::CreateWallet, "C", "Create wallet"),
                (Button::Restore, "R", "Restore wallet"),
                (Button::NewAddress, "N", "New address"),
                (Button::CreateRequest, "U", "Payment request"),
                (Button::Accounts, "A", "Accounts"),
                (Button::RefreshWallet, "F", "Refresh wallet"),
                (Button::CloseWallet, "X", "Close wallet"),
            ]);
            screens::chip_row(
                m,
                Rect::new(
                    body.x,
                    body.y + 2,
                    body.width,
                    body.height.saturating_sub(2),
                ),
                &buttons,
                frame,
            );
        }
        Screen::Log => screens::log_screen(m, body, frame),
        Screen::Node => {
            line(m, "Node connection details hidden", body, frame);
            screens::chip_row(
                m,
                Rect::new(
                    body.x,
                    body.y + 2,
                    body.width,
                    body.height.saturating_sub(2),
                ),
                &[
                    (Button::NodeUp, "u", "Start node"),
                    (Button::NodeDown, "d", "Stop node"),
                    (Button::AddNode, "a", "Add node"),
                    (Button::UseLocal, "l", "Use local node"),
                ],
                frame,
            );
        }
    }
    let status = if !m.policy_current() {
        "Settings unavailable · privacy locked until settings can be read"
    } else if m.has_pending() {
        "Payment outcome unresolved · Wallet (P) clears only after verification"
    } else if m.action.is_some() {
        "Working… Esc requests cancellation"
    } else if m.note_err {
        "Action needs attention · details hidden"
    } else if !m.note.is_empty() {
        "Action status updated · details hidden"
    } else {
        "Tab: screens · input keeps its keys"
    };
    line(
        m,
        status,
        Rect::new(b.x, b.bottom().saturating_sub(2), b.width, 1),
        frame,
    );
    toggle(
        m,
        Rect::new(b.x, b.bottom().saturating_sub(1), 15.min(b.width), 1),
        frame,
    );
    if m.show_help {
        screens::help(m, frame);
        return;
    }
    if let Some(modal) = &m.modal {
        modal_view(m, modal, frame);
    }
    if let send::Stage::Built(plan) = &m.send.stage {
        if m.policy_current() && m.send.review_revealed {
            send::view_confirm(m, plan, frame);
        } else if let Some(inner) = screens::overlay(m, frame, 64, 10, " Private payment review ") {
            m.hits.modal(inner);
            line(m, "Reveal this immutable payment to review its exact recipient, amount and fee. Shared privacy stays on.", inner, frame);
            let reveal = Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1);
            Paragraph::new(if m.policy_current() {
                "(V) Reveal this payment"
            } else {
                "Settings unavailable. Review stays hidden."
            })
            .style(m.theme.warn())
            .render(reveal, frame);
            if m.policy_current() {
                m.hits.button(Button::RevealPayment, reveal);
            }
            screens::modal_actions(m, inner, "Review required", frame);
        }
    }
}

fn modal_view(m: &Tui, modal: &Modal, frame: &mut Frame) {
    if let Modal::CreateWallet(form) = modal {
        screens::create_modal(m, form, frame);
        return;
    }
    if let Modal::Native(review) = modal {
        super::native::view(m, review, frame);
        return;
    }
    if let Modal::Result(result) = modal {
        super::advanced::result_view(m, result, frame);
        return;
    }
    if let Modal::Accounts(panel) = modal {
        super::accounts::view(m, panel, frame);
        return;
    }
    if let Modal::AccountEditor(editor) = modal {
        super::accounts::editor_view(m, editor, frame);
        return;
    }
    if let Modal::PaymentRequest(review) = modal {
        super::requests::view(m, review, frame);
        return;
    }
    if let Modal::Workflow(form) = modal {
        super::workflows::form(m, form, frame);
        return;
    }
    if let Modal::SeedBackup { lines } = modal {
        screens::seed_modal(m, lines, frame);
        return;
    }
    if matches!(modal, Modal::OpenWallet { names, .. } if names.is_empty()) {
        screens::empty_open_modal(m, frame);
        return;
    }
    let Some(inner) = screens::overlay(m, frame, 68, 18, " Protected wallet action ") else {
        return;
    };
    m.hits.modal(inner);
    match modal {
        Modal::OpenWallet { names, sel, pass, asking } => {
            let count = names.len().min(inner.height.saturating_sub(3) as usize);
            let offset = sel.saturating_sub(count.saturating_sub(1));
            for (slot, i) in (offset..names.len()).take(count).enumerate() {
                line(m, &format!("{} Wallet {} · name hidden", if i == *sel { ">" } else { " " }, i + 1), Rect::new(inner.x, inner.y + slot as u16, inner.width, 1), frame);
            }
            m.hits.pane(HitPane::ModalList, inner, inner.y, count as u16, offset);
            if *asking {
                let field = Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1);
                pass.masked_input(&m.theme).render(field, frame); m.hits.field(0, field);
            }
        }
        Modal::CreateWallet(_) => unreachable!(),
        Modal::Prompt { input, field, .. } => {
            line(m, field, inner, frame);
            let area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            masked(m, input, area, frame); m.hits.field(0, area);
        }
        Modal::AddNode { input } => {
            line(m, "Node address (hidden)", inner, frame);
            let area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            masked(m, input, area, frame); m.hits.field(0, area);
        }
        Modal::Qr { .. } => line(m, "Address and QR hidden while shared privacy is on.", inner, frame),
        Modal::SeedBackup { .. } => line(m, "Wallet operation finished. Its details are hidden while privacy is on. Check Wallet before retrying an uncertain operation.", inner, frame),
        Modal::Confirm { what } => {
            let text = match what {
                super::Confirm::NodeUp => "Start your configured node?",
                super::Confirm::NodeDown => "Stop your configured node?",
                super::Confirm::NodeLocal => "Switch this wallet to your local node?",
                super::Confirm::LogLevel { .. } => "Change the node logging level?",
                super::Confirm::ClearPending => "Clear the uncertain-payment interlock? This does not cancel a transaction. Verify its result before sending again.",
            };
            screens::dialog_body(m, vec![ftui::text::Line::styled(text, m.theme.text())], inner, frame);
            return;
        }
        Modal::UseNode { .. } => line(m, "Change the configured wallet connection? The node address is hidden by privacy.", inner, frame),
        Modal::Notice { .. } => line(m, "Fetching or probing public nodes makes network requests. Continue?", inner, frame),
        Modal::Native(_) | Modal::Result(_) | Modal::Workflow(_) | Modal::PaymentRequest(_) | Modal::Accounts(_) | Modal::AccountEditor(_) => unreachable!(),
    }
    screens::modal_actions(m, inner, "Continue", frame);
}
