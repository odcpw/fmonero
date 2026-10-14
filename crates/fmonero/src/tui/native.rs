//! Native Monero holds the immutable artifact and process until deliberate approval.
use super::{advanced::ResultView, hit::Button, screens, subs::Action, Modal, Msg, Tui};
use fmonero_core::{
    native_tx, wallet_ops::WalletIdentity, CancelToken, Context, Error, Piconero, SecretString,
    WalletClient,
};
use ftui::layout::Rect;
use ftui::widgets::{input::TextInput, paragraph::Paragraph, Widget};
use ftui::{Cmd, Frame, KeyCode, KeyEvent, Modifiers};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    path::PathBuf,
};

pub(super) enum Held {
    Unsigned(native_tx::HeldUnsigned),
    Signed(native_tx::HeldSigned),
}
impl Held {
    fn review(&self) -> &native_tx::Review {
        match self {
            Self::Unsigned(h) => h.review(),
            Self::Signed(h) => h.review(),
        }
    }
    fn signed(&self) -> bool {
        matches!(self, Self::Signed(_))
    }
}
pub(super) struct Review {
    pub held: Held,
    destination: Option<PathBuf>,
    max_fee: Option<Piconero>,
    typed: String,
    revealed: bool,
    scroll: usize,
    maximum: Cell<usize>,
    width: Cell<u16>,
    seen: RefCell<BTreeSet<usize>>,
    complete: Cell<bool>,
    error: Option<String>,
}
impl Review {
    pub(super) fn revoke_authorization(&mut self) {
        self.revealed = false;
        self.typed.clear();
        self.scroll = 0;
        self.maximum.set(0);
        self.width.set(0);
        self.seen.borrow_mut().clear();
        self.complete.set(false);
        self.error = None;
    }
    fn new(held: Held, destination: Option<PathBuf>, max_fee: Option<Piconero>) -> Self {
        Self {
            held,
            destination,
            max_fee,
            typed: String::new(),
            revealed: false,
            scroll: 0,
            maximum: Cell::new(0),
            width: Cell::new(0),
            seen: RefCell::new(BTreeSet::new()),
            complete: Cell::new(false),
            error: None,
        }
    }
}
pub(super) enum Job {
    InspectPending {
        expected: native_tx::PendingSubmission,
        password: SecretString,
    },
    Begin {
        wallet: WalletIdentity,
        password: SecretString,
        source: PathBuf,
        destination: Option<PathBuf>,
        max_fee: Option<Piconero>,
        signed: bool,
    },
    Decline(Held),
    Sign(native_tx::HeldUnsigned, PathBuf),
    Submit(native_tx::HeldSigned, String, Option<Piconero>),
}
pub(super) enum Reply {
    Held(Box<Review>),
    Result(ResultView),
}
impl Job {
    pub fn held(&self) -> bool {
        matches!(self, Self::Decline(_) | Self::Sign(..) | Self::Submit(..))
    }
    pub fn submitting(&self) -> bool {
        matches!(self, Self::Submit(..))
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::InspectPending { .. } => "Reopening recorded wallet for inspection",
            Self::Begin { signed: true, .. } => "Loading signed artifact for review",
            Self::Begin { .. } => "Loading unsigned artifact for review",
            Self::Decline(_) => "Declining review and reopening wallet",
            Self::Sign(..) => "Signing reviewed artifact",
            Self::Submit(..) => "Submitting reviewed artifact once",
        }
    }
    pub fn execute(self, ctx: &Context, cancel: &CancelToken) -> Result<Reply, Error> {
        let operation_ctx = ctx.clone().with_wallet_probe_at_refresh();
        let ctx = &operation_ctx;
        let w = WalletClient::from_ctx(ctx)?;
        let result = match self {
            Self::InspectPending { expected, password } => {
                let returned =
                    native_tx::reopen_pending_for_inspection(ctx, &w, &expected, password, cancel)?;
                let mut result = ResultView::complete("Pending wallet inspection", vec!["The native submission record stays intact. Payments and guarded mutations remain blocked. Inspect history on the Wallet page before acknowledging uncertainty; never automatically resubmit the artifact.".into()]);
                returned_state(&mut result, returned);
                result
            }
            Self::Begin {
                wallet,
                password,
                source,
                destination,
                max_fee,
                signed,
            } => {
                let held = if signed {
                    Held::Signed(native_tx::review_signed(
                        ctx, &w, &wallet, password, &source, cancel,
                    )?)
                } else {
                    Held::Unsigned(native_tx::review_unsigned(
                        ctx, &w, &wallet, password, &source, cancel,
                    )?)
                };
                return Ok(Reply::Held(Box::new(Review::new(
                    held,
                    destination,
                    max_fee,
                ))));
            }
            Self::Decline(held) => {
                let returned = match held {
                    Held::Unsigned(h) => h.decline(&w, cancel)?,
                    Held::Signed(h) => h.decline(&w, cancel)?,
                };
                let mut result =
                    ResultView::complete("Review declined", vec!["No approval was sent.".into()]);
                returned_state(&mut result, returned);
                result
            }
            Self::Sign(held, path) => {
                let signed = held.sign(&w, &path, cancel)?;
                let mut result = ResultView::complete("Signed artifact saved", vec![signed.artifact.path.display().to_string(), format!("SHA-256 {}", signed.artifact.sha256), "Nothing was broadcast. Move this private signed artifact to the submitting wallet.".into()]);
                result.lines.extend(signed.txids);
                returned_state(&mut result, signed.returned);
                result
            }
            Self::Submit(held, typed, max_fee) => {
                let submitted = held.submit(&w, &typed, max_fee, cancel)?;
                let mut result = ResultView::complete(if submitted.all_submitted { "Submission evidence received" } else { "Submission outcome unresolved" }, vec![format!("Proven transaction IDs: {}\nUnknown transaction IDs: {}", submitted.txids.len(), submitted.unresolved), "Never automatically resubmit this artifact. Inspect wallet history and the pending record.".into()]);
                result.failed = !submitted.all_submitted;
                result.lines.extend(submitted.txids);
                if let Some(warning) = submitted.warning {
                    result.lines.push(warning);
                    result.failed = true;
                }
                returned_state(&mut result, submitted.returned);
                result
            }
        };
        Ok(Reply::Result(result))
    }
}
fn returned_state(result: &mut ResultView, returned: native_tx::NativeReturn) {
    result.lines.push(if returned.wallet_reopened {
        "Original wallet reopened.".into()
    } else {
        "Original wallet was not reopened. If a native pending record remains, use Tools → Offline → Reopen pending wallet for inspection.".into()
    });
    result.failed |= !returned.wallet_reopened;
    if let Some(warning) = returned.warning {
        result.lines.push(warning);
        result.failed = true;
    }
}
impl Tui {
    pub(super) fn native_key(&mut self, mut review: Box<Review>, key: KeyEvent) -> Cmd<Msg> {
        if key.code == KeyCode::Escape {
            return self.start(Action::Native(Box::new(Job::Decline(review.held))));
        }
        if !self.policy_current() {
            self.modal = Some(Modal::Native(review));
            return Cmd::none();
        }
        if self.privacy() && !review.revealed {
            if matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V')) {
                review.revealed = true;
            }
            self.modal = Some(Modal::Native(review));
            return Cmd::none();
        }
        match key.code {
            KeyCode::Down | KeyCode::PageDown => {
                review.scroll = review.scroll.saturating_add(1).min(review.maximum.get())
            }
            KeyCode::Up | KeyCode::PageUp => review.scroll = review.scroll.saturating_sub(1),
            KeyCode::Backspace => {
                review.typed.pop();
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(Modifiers::CTRL | Modifiers::ALT)
                    && !c.is_control() =>
            {
                if review.typed.len() < 16 {
                    review.typed.push(c);
                }
            }
            KeyCode::Enter => {
                let expected = if review.held.signed() { "send" } else { "sign" };
                if !review.complete.get() {
                    review.error =
                        Some("Read every review row; scroll with ↑↓ before confirming.".into());
                } else if review.typed != expected {
                    review.error = Some(format!("Type {expected} to confirm this exact artifact."));
                } else if review
                    .max_fee
                    .is_some_and(|max| review.held.review().fee > max)
                {
                    review.error = Some(
                        "Fee exceeds your ceiling. Decline this review to change the form.".into(),
                    );
                } else if !review.held.signed() && review.destination.is_none() {
                    review.error = Some("No output destination is attached to this review. Decline and select the artifact destination again.".into());
                } else {
                    let job = match review.held {
                        Held::Unsigned(held) => {
                            Job::Sign(held, review.destination.unwrap_or_default())
                        }
                        Held::Signed(held) => Job::Submit(held, review.typed, review.max_fee),
                    };
                    return self.start(Action::Native(Box::new(job)));
                }
            }
            _ => {}
        }
        self.modal = Some(Modal::Native(review));
        Cmd::none()
    }
}
pub(super) fn view(m: &Tui, state: &Review, frame: &mut Frame) {
    state.complete.set(false);
    let Some(inner) = screens::overlay(
        m,
        frame,
        92,
        frame.bounds().height.saturating_sub(1),
        if state.held.signed() {
            " Submit signed artifact · sends money "
        } else {
            " Sign unsigned artifact · offline "
        },
    ) else {
        return;
    };
    m.hits.modal(inner);
    if !m.policy_current() {
        Paragraph::new(
            "Settings unavailable. Review is hidden. Esc declines and reopens the wallet.",
        )
        .wrap(ftui::text::wrap::WrapMode::Word)
        .style(m.theme.warn())
        .render(inner, frame);
        screens::modal_actions(m, inner, "Review hidden", frame);
        return;
    }
    if m.privacy() && !state.revealed {
        Paragraph::new("Reveal this immutable payment review locally before approval. Shared privacy stays on.").style(m.theme.warn()).render(Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(2)), frame);
        screens::chip_row(
            m,
            Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
            &[(Button::NativeReveal, "V", "Reveal this review")],
            frame,
        );
        screens::modal_actions(m, inner, "Review hidden", frame);
        return;
    }
    let review = state.held.review();
    let mut lines = vec![format!(
        "Artifact SHA-256\n{}\nTransactions {} · minimum ring size {}",
        review.artifact_sha256, review.transactions, review.minimum_ring_size
    )];
    for recipient in &review.recipients {
        lines.push(format!(
            "Recipient\n{}\nAmount {} XMR",
            recipient.address,
            recipient.amount.to_xmr_min(12)
        ));
    }
    lines.push(format!(
        "Input {} XMR\nFee {} XMR\nChange {} XMR\nChange address {}",
        review.input_amount.to_xmr_min(12),
        review.fee.to_xmr_min(12),
        review.change_amount.to_xmr_min(12),
        review.change_address.as_deref().unwrap_or("none")
    ));
    if let Some(max) = state.max_fee {
        lines.push(format!("Fee ceiling {} XMR", max.to_xmr_min(12)));
    }
    if let Some(path) = &state.destination {
        lines.push(format!("New signed output file\n{}", path.display()));
    }
    lines.push(if state.held.signed() { "Approval sends once. Expected transaction IDs are unknown until Monero reports submission. Uncertainty is persisted before approval.".into() } else { "Approval signs this artifact offline. Nothing is broadcast.".into() });
    let rows = lines
        .iter()
        .flat_map(|line| {
            line.lines()
                .flat_map(|s| screens::wrapped(s, inner.width as usize))
        })
        .collect::<Vec<_>>();
    if state.width.replace(inner.width) != inner.width {
        state.seen.borrow_mut().clear();
    }
    let room = inner.height.saturating_sub(4) as usize;
    state.maximum.set(rows.len().saturating_sub(room.max(1)));
    for (slot, (index, line)) in rows
        .iter()
        .enumerate()
        .skip(state.scroll.min(state.maximum.get()))
        .take(room)
        .enumerate()
    {
        Paragraph::new(line.as_str()).style(m.theme.text()).render(
            Rect::new(inner.x, inner.y + slot as u16, inner.width, 1),
            frame,
        );
        state.seen.borrow_mut().insert(index);
    }
    state
        .complete
        .set(room > 0 && state.seen.borrow().len() == rows.len());
    let y = inner.bottom().saturating_sub(4);
    screens::chip_row(
        m,
        Rect::new(inner.x, y, inner.width, 1),
        &[
            (Button::ResultUp, "↑", "Previous"),
            (Button::ResultDown, "↓", "More"),
        ],
        frame,
    );
    let word = if state.held.signed() { "send" } else { "sign" };
    let label = format!("Type {word}: ");
    Paragraph::new(label.as_str())
        .style(m.theme.warn())
        .render(Rect::new(inner.x, y + 1, 11, 1), frame);
    TextInput::new()
        .with_value(state.typed.clone())
        .with_focused(true)
        .render(
            Rect::new(inner.x + 11, y + 1, inner.width.saturating_sub(11), 1),
            frame,
        );
    // The typed confirmation is always the active field; clicking it consumes the click.
    m.hits.button(
        Button::NativeFocus,
        Rect::new(inner.x + 11, y + 1, inner.width.saturating_sub(11), 1),
    );
    Paragraph::new(state.error.as_deref().unwrap_or(if state.complete.get() {
        "All review rows displayed. Esc declines and reopens the wallet."
    } else {
        "Scroll through all review rows before approval."
    }))
    .style(m.theme.warn())
    .render(Rect::new(inner.x, y + 2, inner.width, 1), frame);
    screens::modal_actions(
        m,
        inner,
        if state.held.signed() {
            "Submit once"
        } else {
            "Sign and save"
        },
        frame,
    );
}
