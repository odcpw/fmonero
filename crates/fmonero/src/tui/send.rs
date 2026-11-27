//! The Send screen and its state machine (DESIGN.md §7a) — the load-bearing screen.
//!
//! ```text
//! Form ──validate──▶ Validating ──▶ Building ──▶ Built ──type "send"+Enter──▶ Relaying ──▶ Sent
//!   ▲                    │             │           │                             │          │
//!   └──── edit ──────────┘   Esc ──────┘           └── Esc: discard ─────────────┘          └─▶ Form
//! ```
//!
//! Both exact-amount and send-all requests use core preparation and immutable review.
//! `send::build` and `send::relay` one `SendRequest` and the plan they returned (§2a.3).

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use ftui::layout::{Constraint, Flex, Rect};
use ftui::text::wrap::WrapMode;
use ftui::text::{Line, Span, Text};
use ftui::widgets::input::TextInput;
use ftui::widgets::paragraph::Paragraph;
use ftui::widgets::Widget;
use ftui::{Event, KeyCode, KeyEvent};

use unicode_width::UnicodeWidthStr;

use fmonero_core::rpc::wallet::BookEntry;
use fmonero_core::{address, send::Plan, Piconero, SendRequest};

use super::screens::{fit, framed, short_addr, wrapped, Tier};
use super::theme::Theme;
use super::{next_fee_level, prev_fee_level, Tui};

/// Typing this word is the TUI's `--yes` (§7a): the modal needs the word, not just Enter.
pub(super) const CONFIRM_WORD: &str = "send";

/// The Tab cycle. `Priority` is a field rather than a bare shortcut so that `p` can cycle it
/// without being swallowed by an address — base58 contains `p`, and `Ctrl+P` cycles it from
/// anywhere (both are in the §7a keymap table).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Field {
    To,
    Amount,
    Note,
    Priority,
    Account,
    FeeLimit,
}

pub(super) enum Stage {
    Form,
    /// One `validate_address` in a `Cmd::task` (§7a step 2).
    Validating,
    /// `ActionSub(Action::BuildTx)` → `send::build`, which builds once with
    /// `do_not_relay: true, get_tx_metadata: true`.
    Building,
    /// The confirm modal, holding the plan `send::relay` will consume.
    Built(Box<Plan>),
    Relaying,
    /// Successful relay; the next key returns to the form.
    Sent,
}

/// What a key press asks the model to do. The model owns every `Cmd` and every action.
pub(super) enum SendCmd {
    None,
    /// Run `validate_address` for this address, then build.
    Validate,
    Sweep(fmonero_core::send::SweepRequest),
    /// Start `Action::Relay` with the plan the modal is showing.
    Relay,
    /// Load the address book for the `@` picker.
    Book,
    /// A refusal for the status line.
    Note(String),
    /// A plain note for the status line.
    Info(String),
}

pub(super) struct SendScreen {
    pub to: TextInput,
    pub amount: TextInput,
    pub note: TextInput,
    pub fee_limit: TextInput,
    pub field: Field,
    /// 0–4, defaulting to `settings.wallet.priority`. `0` is an **alias of the lowest
    /// tier**, not a "wallet default" (§1b.4), and the chip row says so.
    pub priority: u32,
    /// §7a's account selector: the account the transaction is built from, defaulting to
    /// `settings.wallet.account` and bounded by `get_accounts` (§5's `wallet.accounts`).
    pub account: u32,
    pub stage: Stage,
    /// The confirm modal's typed word.
    pub typed: String,
    pub review_revealed: bool,
    pub review_offset: usize,
    pub review_max: Cell<usize>,
    pub review_visible: Cell<bool>,
    review_width: Cell<u16>,
    review_seen: RefCell<BTreeSet<usize>>,
    /// The `@` picker: the address book and the selected row.
    pub picker: Option<(Vec<BookEntry>, usize)>,
    /// The form has the keys. `Esc` leaves it so that `1 2 3 4`, `q` and `?` work; `Enter`
    /// returns to it. Without this an address typed into the To field would switch screens.
    pub editing: bool,
    /// **Round 5, S12: `Tab` was pressed while a built transaction was on screen.**
    ///
    /// `switch_screen` raises `Press Esc first to discard this.` as a toast, and `toast_hidden`
    /// suppresses every toast while the stage is `Built` — so the one modal that needs that sentence
    /// was the one that could not show it. Pressing `Tab` in the send confirm moved nothing and said
    /// nothing, at 0.2 s and at 1.5 s, while the help overlay's first row promised `Tab` always
    /// works "in a pop-up". The flag is set by `switch_screen` and drawn by `view_confirm` as a row
    /// of the modal itself, which is where §7a.1 says the exception belongs.
    pub tab_note: bool,
}

impl SendScreen {
    pub fn new(th: &Theme, priority: u32, account: u32) -> SendScreen {
        SendScreen {
            to: field_input(th),
            amount: field_input(th),
            note: field_input(th),
            fee_limit: field_input(th),
            field: Field::To,
            priority,
            account,
            stage: Stage::Form,
            typed: String::new(),
            review_revealed: false,
            review_offset: 0,
            review_max: Cell::new(0),
            review_visible: Cell::new(false),
            review_width: Cell::new(0),
            review_seen: RefCell::new(BTreeSet::new()),
            picker: None,
            editing: false,
            tab_note: false,
        }
    }

    pub fn restyle(&mut self, th: &Theme) {
        for input in [
            &mut self.to,
            &mut self.amount,
            &mut self.note,
            &mut self.fee_limit,
        ] {
            let mut fresh = field_input(th).with_value(input.value().to_string());
            fresh.set_focused(input.focused());
            *input = fresh;
        }
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.stage,
            Stage::Validating | Stage::Building | Stage::Relaying
        )
    }

    /// The confirm modal is open, so it owns every key.
    pub fn confirming(&self) -> bool {
        matches!(self.stage, Stage::Built(_))
    }

    pub fn reset_review(&mut self) {
        self.review_revealed = false;
        self.review_offset = 0;
        self.review_visible.set(false);
        self.review_seen.borrow_mut().clear();
        self.review_width.set(0);
    }

    pub fn back_to_form(&mut self) {
        self.reset_review();
        self.stage = Stage::Form;
        self.typed.clear();
        // Round 5, S12: the notice belongs to one confirm, and leaving it takes it away.
        self.tab_note = false;
    }

    pub fn clear(&mut self) {
        self.to.clear();
        self.amount.clear();
        self.note.clear();
        self.fee_limit.clear();
        self.field = Field::To;
        self.back_to_form();
    }

    /// The live validation line: `(text, ok)`. Each keystroke re-runs `address::parse` and
    /// `Piconero::parse` locally — nothing is sent (§7a step 1).
    pub fn validation(&self) -> (String, bool) {
        let to = self.to.value().trim();
        let amount = self.amount.value().trim();
        if to.is_empty() && amount.is_empty() {
            // **Not `Enter who to pay …`** (round 4 iteration 3): this row is about filling the
            // form, and opening it with the name of the key that submits it was one of five
            // wordings for Enter on one screen.
            return ("Type who to pay and how much.".into(), false);
        }
        let dest = if let Some(label) = to.strip_prefix('@') {
            if label.is_empty() {
                return ("Type a name after @.".into(), false);
            }
            format!("address book: {label}")
        } else {
            match address::parse(to) {
                // `mainnet standard` was two wire words where the sentence wants one phrase
                // (self-read, round 3). The network stays: sending to the wrong one is the
                // mistake this line exists to catch.
                Ok(a) => format!("{} {}", a.net, a.kind.words()),
                Err(e) => return (e.to_string(), false),
            }
        };
        // **M18: an empty Amount is not a mistake, it is the next thing to type.** With the cursor
        // still in `To` and nothing typed into `Amount`, this line read `"" is not an amount. You
        // typed nothing.` — an empty quoted string presented as something he had typed, about a
        // field two rows below the cursor. `Piconero::parse`'s message is right for a CLI argument
        // that was actually supplied and wrong for a form field nobody has reached.
        if amount.is_empty() {
            return (format!("{dest} · now type how much, in XMR."), false);
        }
        if amount.eq_ignore_ascii_case("all") {
            return (
                "Send all unlocked funds from this account, less the exact reviewed fee.".into(),
                true,
            );
        }
        match Piconero::parse(amount) {
            Ok(p) if p.0 == 0 => ("0 XMR is not an amount.".into(), false),
            // **M19: `the check` is named.** Three lines on this screen used the noun and none of
            // them said what it is — the wallet service works out the fee by preparing the payment
            // without sending it, which is the one thing a person needs to know before pressing a
            // key on a money screen.
            Ok(p) => (
                format!(
                    "{dest} · {} XMR · Enter works out the fee. Nothing is sent yet.",
                    p.to_xmr_money()
                ),
                true,
            ),
            Err(e) => (e.to_string(), false),
        }
    }

    /// The request the form describes, or the reason it is not one.
    pub fn request(&self, max_fee: Option<Piconero>) -> Result<SendRequest, String> {
        let (text, ok) = self.validation();
        if !ok {
            return Err(text);
        }
        let amount = Piconero::parse(self.amount.value().trim()).map_err(|e| e.to_string())?;
        let note = self.note.value().trim();
        Ok(SendRequest {
            dest: self.to.value().trim().to_string(),
            amount,
            account_index: self.account,
            priority: self.priority,
            note: (!note.is_empty()).then(|| note.to_string()),
            max_fee: max_fee.or(self.max_fee()?),
        })
    }

    fn max_fee(&self) -> Result<Option<Piconero>, String> {
        let text = self.fee_limit.value().trim();
        if text.is_empty() {
            Ok(None)
        } else {
            Piconero::parse(text).map(Some).map_err(|e| e.to_string())
        }
    }
    fn sweep_request(&self) -> Result<fmonero_core::send::SweepRequest, String> {
        let (why, valid) = self.validation();
        if !valid {
            return Err(why);
        }
        Ok(fmonero_core::send::SweepRequest {
            dest: self.to.value().trim().into(),
            account_index: self.account,
            priority: self.priority,
            below: None,
            max_fee: self.max_fee()?,
        })
    }
    /// Focus the form field a click landed on (§7a.2), by its index in [`FIELDS`] order.
    pub fn focus_index(&mut self, index: usize) {
        let field = match index {
            0 => Field::To,
            1 => Field::Amount,
            2 => Field::Note,
            3 => Field::Priority,
            4 => Field::Account,
            _ => Field::FeeLimit,
        };
        self.editing = true;
        self.focus(field);
    }

    /// The form, its picker and its confirm modal own every key while they are focused.
    /// While an action is in flight they own none, so `Esc` reaches the model and cancels it.
    pub fn owns_keys(&self) -> bool {
        self.confirming() || self.picker.is_some() || (self.editing && !self.busy())
    }

    pub fn on_paste(&mut self, event: &Event) {
        if self.editing
            && matches!(self.stage, Stage::Form)
            && self.picker.is_none()
            && matches!(
                self.field,
                Field::To | Field::Amount | Field::Note | Field::FeeLimit
            )
        {
            self.focused_input_mut().handle_event(event);
        }
    }

    /// `accounts` is how many the wallet has (§5's `wallet.accounts`), so the selector
    /// cannot name one that does not exist.
    pub fn on_key(&mut self, k: KeyEvent, accounts: u32) -> SendCmd {
        if self.confirming() {
            return self.on_confirm_key(k);
        }
        if self.picker.is_some() {
            return self.on_picker_key(k);
        }
        if let Stage::Sent = self.stage {
            self.back_to_form();
        }
        let ctrl = k.modifiers.contains(ftui::Modifiers::CTRL);
        match k.code {
            KeyCode::Escape => {
                self.editing = false;
                self.blur();
                SendCmd::Info("You left the form. 1-6 change screens, Enter goes back.".into())
            }
            KeyCode::Char('p') if ctrl => {
                self.priority = next_fee_level(self.priority);
                SendCmd::None
            }
            KeyCode::Tab | KeyCode::Down => {
                self.focus(match self.field {
                    Field::To => Field::Amount,
                    Field::Amount => Field::Note,
                    Field::Note => Field::Priority,
                    Field::Priority => Field::Account,
                    Field::Account => Field::FeeLimit,
                    Field::FeeLimit => Field::To,
                });
                SendCmd::None
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus(match self.field {
                    Field::To => Field::FeeLimit,
                    Field::Amount => Field::To,
                    Field::Note => Field::Amount,
                    Field::Priority => Field::Note,
                    Field::Account => Field::Priority,
                    Field::FeeLimit => Field::Account,
                });
                SendCmd::None
            }
            KeyCode::Enter if self.amount.value().trim().eq_ignore_ascii_case("all") => {
                match self.sweep_request() {
                    Ok(request) => {
                        self.stage = Stage::Building;
                        SendCmd::Sweep(request)
                    }
                    Err(why) => SendCmd::Note(why),
                }
            }
            KeyCode::Enter => match self.request(None) {
                Ok(_) => {
                    self.stage = Stage::Validating;
                    SendCmd::Validate
                }
                Err(why) => SendCmd::Note(why),
            },
            code if self.field == Field::Account => {
                let n = accounts.max(1);
                match code {
                    KeyCode::Right => self.account = (self.account + 1) % n,
                    KeyCode::Left => self.account = (self.account + n - 1) % n,
                    KeyCode::Char(c @ '0'..='9') => {
                        let want = u32::from(c as u8 - b'0');
                        if want < n {
                            self.account = want;
                        }
                    }
                    _ => {}
                }
                SendCmd::None
            }
            code if self.field == Field::Priority => {
                match code {
                    KeyCode::Char('p') | KeyCode::Right => {
                        self.priority = next_fee_level(self.priority)
                    }
                    KeyCode::Left => self.priority = prev_fee_level(self.priority),
                    KeyCode::Char(c @ '0'..='4') => {
                        self.priority = u32::from(c as u8 - b'0');
                    }
                    _ => {}
                }
                SendCmd::None
            }
            KeyCode::Char('b') if ctrl => SendCmd::Book,
            _ => {
                self.focused_input_mut().handle_event(&Event::Key(k));
                SendCmd::None
            }
        }
    }

    fn on_picker_key(&mut self, k: KeyEvent) -> SendCmd {
        let Some((entries, sel)) = self.picker.as_mut() else {
            return SendCmd::None;
        };
        match k.code {
            KeyCode::Escape => self.picker = None,
            KeyCode::Down => *sel = (*sel + 1).min(entries.len().saturating_sub(1)),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Enter => {
                // The picker yields a LITERAL address, so `send::build`'s step 5a never sees
                // a label from the TUI (§4).
                if let Some(e) = entries.get(*sel) {
                    let address = e.address.clone();
                    self.to.set_value(address);
                }
                self.picker = None;
            }
            _ => {}
        }
        SendCmd::None
    }

    /// The confirm modal is the whole safety surface: it needs the word, not just Enter.
    fn on_confirm_key(&mut self, k: KeyEvent) -> SendCmd {
        match k.code {
            KeyCode::PageDown | KeyCode::Down => { self.review_offset = self.review_offset.saturating_add(1).min(self.review_max.get()); SendCmd::None }
            KeyCode::PageUp | KeyCode::Up => { self.review_offset = self.review_offset.saturating_sub(1); SendCmd::None }
            KeyCode::Tab | KeyCode::BackTab => SendCmd::None,
            KeyCode::Enter if !self.review_visible.get() => SendCmd::Note("Read the complete payment review before confirming. Scroll with ↑↓ or enlarge the window.".into()),
            KeyCode::Escape => {
                // Discarding drops the `Plan`, which releases the §2a.11 `send.lock` and
                // leaves nothing to clean up: `do_not_relay` never committed the tx.
                self.back_to_form();
                SendCmd::Note("Discarded. Nothing was sent.".into())
            }
            KeyCode::Backspace => {
                self.typed.pop();
                SendCmd::None
            }
            KeyCode::Char(c) => {
                if self.typed.len() < 16 && !c.is_control() { self.typed.push(c); }
                SendCmd::None
            }
            // A plan whose `amount + fee` overflows `u64` has no total to confirm, and the modal
            // renders `—` for it (§5's shape rules). Confirming a sum that was never computed is
            // the one thing this screen exists to prevent, so the word is refused.
            KeyCode::Enter if matches!(&self.stage, Stage::Built(p) if p.total().is_err()) => {
                SendCmd::Note("That total is too large to add up. Esc discards it.".into())
            }
            KeyCode::Enter if self.typed.trim() == CONFIRM_WORD => SendCmd::Relay,
            KeyCode::Enter => SendCmd::Note(format!("Type \"{CONFIRM_WORD}\" to confirm.")),
            _ => SendCmd::None,
        }
    }

    /// The form takes the keys again (entering the screen, or `Enter` after `Esc`).
    pub fn resume(&mut self) {
        self.editing = true;
        let f = self.field;
        self.focus(f);
    }

    fn blur(&mut self) {
        self.to.set_focused(false);
        self.amount.set_focused(false);
        self.note.set_focused(false);
        self.fee_limit.set_focused(false);
    }

    pub fn focus(&mut self, f: Field) {
        self.to.set_focused(f == Field::To);
        self.amount.set_focused(f == Field::Amount);
        self.note.set_focused(f == Field::Note);
        self.fee_limit.set_focused(f == Field::FeeLimit);
        self.field = f;
    }

    fn focused_input_mut(&mut self) -> &mut TextInput {
        match self.field {
            Field::Amount => &mut self.amount,
            Field::Note => &mut self.note,
            Field::FeeLimit => &mut self.fee_limit,
            _ => &mut self.to,
        }
    }
}

fn field_input(th: &Theme) -> TextInput {
    TextInput::new()
        .with_max_length(16 * 1024)
        .with_style(th.text())
        .with_cursor_style(th.cursor())
}

// ---------------------------------------------------------------- render

pub(super) fn view(m: &Tui, area: Rect, frame: &mut ftui::Frame, tier: Tier) {
    let s = &m.send;
    let th = &m.theme;
    let (body, preview) = if tier == Tier::Wide && !m.privacy() {
        let cols = Flex::horizontal()
            .constraints([Constraint::Percentage(60.0), Constraint::Fill])
            .split(area);
        (cols[0], Some(cols[1]))
    } else {
        (area, None)
    };
    let Some(inner) = framed(
        body,
        th,
        " Send · prepare, review, then confirm ",
        s.editing,
        frame,
    ) else {
        return;
    };
    if inner.height < 3 {
        return;
    }
    let focus: usize = match s.field {
        Field::To => 0,
        Field::Amount => 1,
        Field::Note => 2,
        Field::Priority => 3,
        Field::Account => 4,
        Field::FeeLimit => 5,
    };
    let room = inner.height.saturating_sub(3).max(1) as usize;
    let offset = focus.saturating_sub(room.saturating_sub(1));
    for (slot, index) in (offset..6).take(room).enumerate() {
        let row = Rect::new(inner.x, inner.y + slot as u16, inner.width, 1);
        let label = [
            "Recipient",
            "Amount / all",
            "Note",
            "Fee level",
            "Account",
            "Fee ceiling",
        ][index];
        let label_w = 14.min(row.width);
        Paragraph::new(format!("{}{label}", if focus == index { ">" } else { " " }))
            .style(if focus == index {
                th.accented()
            } else {
                th.dimmed()
            })
            .render(Rect::new(row.x, row.y, label_w, 1), frame);
        let field = Rect::new(row.x + label_w, row.y, row.width.saturating_sub(label_w), 1);
        if index == 3 || index == 4 {
            let value = if index == 3 {
                super::fee_level_name(s.priority).to_string()
            } else {
                s.account.to_string()
            };
            Paragraph::new(format!("◂ {value} ▸"))
                .style(th.text())
                .render(field, frame);
            let which = if index == 3 {
                super::hit::Selector::Priority
            } else {
                super::hit::Selector::Account
            };
            m.hits.arrow(which, -1, Rect::new(field.x, field.y, 1, 1));
            m.hits.arrow(
                which,
                1,
                Rect::new(field.x + value.width() as u16 + 3, field.y, 1, 1),
            );
            m.hits.field(index, Rect::new(row.x, row.y, label_w, 1));
            m.hits.field(
                index,
                Rect::new(field.x + 1, field.y, value.width() as u16 + 2, 1),
            );
        } else {
            let input = match index {
                0 => &s.to,
                1 => &s.amount,
                2 => &s.note,
                _ => &s.fee_limit,
            };
            if m.privacy() {
                let mut masked =
                    field_input(th).with_value("•".repeat(input.value().chars().count()));
                masked.set_focused(input.focused());
                masked.render(field, frame);
            } else {
                super::screens::hinted_input(
                    th,
                    input,
                    if index == 5 { "optional XMR cap" } else { "" },
                    field,
                    frame,
                );
            }
            m.hits.field(index, row);
        }
    }
    let status = match s.stage {
        Stage::Validating => "Checking recipient…".into(),
        Stage::Building => "Preparing exact fee; nothing sent. Esc cancels.".into(),
        Stage::Relaying => "Submission in progress; wait for its result.".into(),
        _ if m.privacy() => "Values hidden · final review needs deliberate local reveal.".into(),
        _ => s.validation().0,
    };
    Paragraph::new(status).style(th.dimmed()).render(
        Rect::new(inner.x, inner.bottom().saturating_sub(3), inner.width, 1),
        frame,
    );
    super::screens::chip_row(
        m,
        Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2),
        &[
            (super::hit::Button::PreparePayment, "Enter", "Review"),
            (super::hit::Button::ParseRequest, "U", "Open URI"),
            (super::hit::Button::SendAll, "A", "Send all"),
            (super::hit::Button::SendBook, "B", "Contacts"),
        ],
        frame,
    );
    if let Some(area) = preview {
        view_preview(m, area, frame);
    }
    if let Some((entries, selected)) = &s.picker {
        view_picker(m, entries, *selected, frame);
    }
}

fn view_preview(m: &Tui, area: Rect, frame: &mut ftui::Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Preview ", false, frame) else {
        return;
    };
    let s = &m.send;
    let mut lines = vec![Line::styled("To".to_string(), th.dimmed())];
    // The recipient is never elided: an elided address is how you send to the wrong one. **And an
    // empty field says so** (round 4's own read): `To` with nothing under it was a label and a blank
    // where every other row of this pane shows `—` for a value it does not have yet (§5).
    if s.to.value().trim().is_empty() {
        lines.push(Line::styled("—".to_string(), th.dimmed()));
    } else {
        lines.extend(
            wrapped(s.to.value(), inner.width as usize)
                .into_iter()
                .map(|l| Line::styled(l, th.text())),
        );
    }
    let amount = Piconero::parse(s.amount.value().trim()).ok();
    lines.push(Line::from_spans([
        Span::styled("Amount  ".to_string(), th.dimmed()),
        Span::styled(
            // **Eight decimals, always** (round 5, M1). Round 4 iteration 3 put the Amount, the Fee
            // and the Total at one precision once a plan existed, and left the pre-plan Amount at
            // four on the argument that it is "the figure the reader typed" — so the number changed
            // shape under him at the moment he pressed Enter: `Amount 0.1000 XMR` became
            // `Amount 0.10000000 XMR`. The reader is deciding about one amount; it has one shape on
            // this path, and `Piconero::to_xmr_money` is where that decision lives.
            match amount {
                Some(a) => format!("{} XMR", a.to_xmr_money()),
                None => "—".to_string(),
            },
            th.text(),
        ),
    ]));
    // **The Preview learns the fee the moment the fee is known** (round 4 iteration 3). It read
    // `Fee  known once Enter prepares it` while the confirm modal beside it, in the same frame,
    // showed `Fee 0.00003072 XMR` — two panes, one fact, one of them wrong. The plan lives on
    // `Stage::Built`, which is exactly the state the modal is drawn from, so there is one source.
    //
    // M19's own point stands for the other states: the noun `the check` was never defined on this
    // screen, and what the fee waits for is the wallet service working it out. One wording for
    // that, here, on the validation row, in the `?` overlay and in the toast.
    let (fee_text, total_text) = match &s.stage {
        Stage::Built(plan) => (
            format!("{} XMR", plan.total_fee.to_xmr_min(Piconero::FEE_DECIMALS)),
            plan.total()
                .ok()
                .map(|t| format!("{} XMR", t.to_xmr_min(Piconero::FEE_DECIMALS))),
        ),
        Stage::Validating | Stage::Building => ("working it out…".to_string(), None),
        _ => ("known once Enter works out the fee".to_string(), None),
    };
    lines.push(Line::from_spans([
        Span::styled("Fee     ".to_string(), th.dimmed()),
        Span::styled(fee_text, th.dimmed()),
    ]));
    if let Some(total) = total_text {
        lines.push(Line::from_spans([
            Span::styled("Total   ".to_string(), th.dimmed()),
            Span::styled(total, th.text()),
        ]));
    }
    lines.push(Line::from_spans([
        Span::styled("Account ".to_string(), th.dimmed()),
        Span::styled(s.account.to_string(), th.text()),
    ]));
    if let Some(w) = m.status.as_ref().map(|st| &st.wallet) {
        lines.push(Line::from_spans([
            Span::styled("Unlocked".to_string(), th.dimmed()),
            // §2o rule 9: an amount carries its unit. This row read `Unlocked 1.234567891234`
            // in the one pane whose subject is how much you can spend (self-read, round 3).
            //
            // **And `—` takes no unit** (round 4 iteration 3): with no wallet open this printed
            // `Unlocked — XMR`, which puts a denomination on a value there is none of. The glyph is
            // the whole cell, as it is everywhere else in the program.
            Span::styled(
                match &w.unlocked_xmr {
                    Some(x) => format!(" {x} XMR"),
                    None => " —".to_string(),
                },
                th.text(),
            ),
        ]));
    }
    Paragraph::new(Text::from_lines(lines))
        .wrap(WrapMode::None)
        .render(inner, frame);
}

fn view_picker(m: &Tui, entries: &[BookEntry], sel: usize, frame: &mut ftui::Frame) {
    let rows: Vec<String> = if entries.is_empty() {
        vec!["No contacts yet. Add one in Tools → Wallet → Accounts and contacts.".into()]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if m.privacy() {
                    format!("Contact {} · details hidden", i + 1)
                } else {
                    format!("{} {}", fit(&e.description, 18), short_addr(&e.address))
                }
            })
            .collect()
    };
    let sel = if entries.is_empty() { usize::MAX } else { sel };
    super::screens::list_modal(m, " Address book · Enter picks ", &rows, sel, None, frame);
}

/// Render the complete payment review with scrolling when it exceeds the terminal height.
/// The review owns its hit targets and records which rows have been shown before confirmation.
pub(super) fn view_confirm(m: &Tui, plan: &Plan, frame: &mut ftui::Frame) {
    let Some(inner) = super::screens::overlay(
        m,
        frame,
        82,
        frame.bounds().height.saturating_sub(2),
        " Review payment · sends money ",
    ) else {
        return;
    };
    m.hits.modal(inner);
    let total = plan
        .total()
        .map(|p| p.to_xmr_min(12))
        .unwrap_or_else(|_| "Invalid total".into());
    let text = format!("Recipient\n{}\nAmount  {} XMR\nFee     {} XMR\nTotal   {} XMR\nAccount {} · Fee level {}\nThe exact prepared transaction will be submitted once.",
        plan.dest, plan.total_amount.to_xmr_min(12), plan.total_fee.to_xmr_min(12), total, plan.account_index, super::fee_level_name(plan.priority));
    let rows: Vec<String> = text
        .lines()
        .flat_map(|l| wrapped(l, inner.width.max(1) as usize))
        .collect();
    if m.send.review_width.replace(inner.width) != inner.width {
        m.send.review_seen.borrow_mut().clear();
    }
    let room = inner.height.saturating_sub(4) as usize;
    m.send
        .review_max
        .set(rows.len().saturating_sub(room.max(1)));
    let offset = m
        .send
        .review_offset
        .min(rows.len().saturating_sub(room.max(1)));
    for (slot, (index, row)) in rows.iter().enumerate().skip(offset).take(room).enumerate() {
        Paragraph::new(row.as_str()).style(m.theme.text()).render(
            Rect::new(inner.x, inner.y + slot as u16, inner.width, 1),
            frame,
        );
        m.send.review_seen.borrow_mut().insert(index);
    }
    let complete =
        room > 0 && m.send.review_seen.borrow().len() == rows.len() && plan.total().is_ok();
    m.send.review_visible.set(complete);
    let y = inner.bottom().saturating_sub(4);
    for (button, label, x) in [
        (super::hit::Button::ReviewUp, "↑ Previous", inner.x),
        (super::hit::Button::ReviewDown, "↓ More", inner.x + 14),
    ] {
        let area = Rect::new(x, y, 12.min(inner.right().saturating_sub(x)), 1);
        Paragraph::new(label)
            .style(m.theme.key())
            .render(area, frame);
        m.hits.button(button, area);
    }
    let prompt = if complete {
        "Type send to confirm"
    } else {
        "Read all rows to enable confirmation"
    };
    Paragraph::new(prompt).style(m.theme.warn()).render(
        Rect::new(inner.x, inner.bottom().saturating_sub(3), inner.width, 1),
        frame,
    );
    Paragraph::new(format!("> {}", m.send.typed))
        .style(m.theme.text())
        .render(
            Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
            frame,
        );
    m.hits.button(
        super::hit::Button::ReviewFocus,
        Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
    );
    super::screens::modal_actions(m, inner, "Send", frame);
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEST: &str = "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm";

    fn screen() -> SendScreen {
        SendScreen::new(&Theme::default(), 2, 0)
    }

    #[test]
    fn all_builds_a_typed_sweep_request_with_the_fee_ceiling() {
        let mut s = screen();
        s.to.set_value(DEST);
        s.amount.set_value("all");
        s.fee_limit.set_value("0.001");
        assert!(s.validation().1);
        let SendCmd::Sweep(request) = s.on_key(key(KeyCode::Enter), 1) else {
            panic!("expected sweep preparation");
        };
        assert_eq!(request.dest, DEST);
        assert_eq!(request.max_fee, Some(Piconero(1_000_000_000)));
        assert!(request.below.is_none());
        assert!(matches!(s.stage, Stage::Building));
    }

    #[test]
    fn a_good_form_becomes_a_send_request_without_a_sweep_field() {
        let mut s = screen();
        s.to.set_value(DEST);
        s.amount.set_value("0.5");
        s.note.set_value("rent");
        let (_, ok) = s.validation();
        assert!(ok);
        s.account = 3;
        let req = s.request(Some(Piconero(1))).expect("a request");
        assert_eq!(req.dest, DEST);
        assert_eq!(req.amount, Piconero(500_000_000_000));
        assert_eq!(req.account_index, 3);
        assert_eq!(req.priority, 2);
        assert_eq!(req.note.as_deref(), Some("rent"));
        assert_eq!(req.max_fee, Some(Piconero(1)));
    }

    #[test]
    fn a_zero_amount_and_a_bad_address_are_both_refused_before_anything_is_sent() {
        let mut s = screen();
        s.to.set_value(DEST);
        s.amount.set_value("0");
        assert_eq!(s.validation().0, "0 XMR is not an amount.");
        s.amount.set_value("0.5");
        s.to.set_value("not-an-address");
        assert!(!s.validation().1);
        s.to.set_value("@");
        assert!(s.validation().0.contains("Type a name after @"));
        s.to.set_value("@rent");
        assert!(s.validation().1, "a label is a legal recipient");
    }

    #[test]
    fn the_confirm_modal_needs_the_word_not_just_enter() {
        let mut s = screen();
        s.typed = "sen".into();
        assert!(matches!(
            s.on_confirm_key(key(KeyCode::Enter)),
            SendCmd::Note(_)
        ));
        s.typed = CONFIRM_WORD.into();
        s.review_visible.set(true);
        assert!(matches!(
            s.on_confirm_key(key(KeyCode::Enter)),
            SendCmd::Relay
        ));
        s.typed.clear();
        assert!(matches!(
            s.on_confirm_key(key(KeyCode::Escape)),
            SendCmd::Note(_)
        ));
        assert!(matches!(s.stage, Stage::Form), "Esc returns to the form");
    }

    #[test]
    fn the_account_selector_stays_inside_the_accounts_the_wallet_has() {
        let mut s = screen();
        s.focus(Field::Account);
        // Two accounts: 0 -> 1 -> 0, and a digit the wallet does not have is ignored.
        let _ = s.on_key(key(KeyCode::Right), 2);
        assert_eq!(s.account, 1);
        let _ = s.on_key(key(KeyCode::Right), 2);
        assert_eq!(s.account, 0);
        let _ = s.on_key(key(KeyCode::Left), 2);
        assert_eq!(s.account, 1);
        let _ = s.on_key(key(KeyCode::Char('7')), 2);
        assert_eq!(s.account, 1, "account 7 does not exist");
        let _ = s.on_key(key(KeyCode::Char('0')), 2);
        assert_eq!(s.account, 0);
        assert_eq!(
            s.request(None).err(),
            Some("Type who to pay and how much.".to_string()),
            "the selector must not build anything by itself"
        );
    }

    #[test]
    fn the_picker_yields_a_literal_address_never_a_label() {
        let mut s = screen();
        s.picker = Some((
            vec![BookEntry {
                index: 0,
                address: DEST.into(),
                description: "rent".into(),
            }],
            0,
        ));
        let _ = s.on_key(key(KeyCode::Enter), 1);
        assert_eq!(s.to.value(), DEST);
        assert!(s.picker.is_none());
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code)
    }
}
