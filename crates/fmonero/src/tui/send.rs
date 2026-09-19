//! The Send screen and its state machine (DESIGN.md §7a) — the load-bearing screen.
//!
//! ```text
//! Form ──validate──▶ Validating ──▶ Building ──▶ Built ──type "send"+Enter──▶ Relaying ──▶ Sent
//!   ▲                    │             │           │                             │          │
//!   └──── edit ──────────┘   Esc ──────┘           └── Esc: discard ─────────────┘          └─▶ Form
//! ```
//!
//! **One transaction only — sweep is CLI-only in v0.1** (§0): `all` in the Amount field is
//! refused inline. The screen never calls `build_transfer` or `relay_tx`: it hands
//! `send::build` and `send::relay` one `SendRequest` and the plan they returned (§2a.3).

use ftui::layout::{Constraint, Flex, Rect};
use ftui::text::wrap::WrapMode;
use ftui::text::{Line, Span, Text};
use ftui::widgets::input::TextInput;
use ftui::widgets::paragraph::Paragraph;
use ftui::widgets::Widget;
use ftui::{Event, KeyCode, KeyEvent, Style};

use unicode_width::UnicodeWidthStr;

use fmonero_core::rpc::wallet::BookEntry;
use fmonero_core::{address, send::Plan, Piconero, SendRequest};

use super::screens::{fit, panel, short_addr, wrapped, Tier};
use super::theme::Theme;
use super::{next_fee_level, prev_fee_level, Tui};

/// Typing this word is the TUI's `--yes` (§7a): the modal needs the word, not just Enter.
pub(super) const CONFIRM_WORD: &str = "send";

/// What the Amount field says when someone types `all`.
///
/// **One of the four places a shell command may appear in the TUI** (§2o.1, `docs/COPY.md` §2),
/// and the only one whose cause is a scope decision rather than an impossibility: §0 keeps
/// sweep out of the TUI in v0.1 and this screen prepares exactly one payment. When sweep enters
/// the TUI this line goes.
pub(super) const NO_SWEEP: &str = "Sweep is not in the TUI yet. To send everything, in a \
                                   terminal: fmonero wallet sweep <address> --dry-run";

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
    /// The toast after a successful relay; any key returns to Form.
    ///
    /// It carried the txid until round 4's S10 deleted every id from the TUI's prose. The
    /// variant carries nothing now: the screen says where the payment is, and `--json` has
    /// the id.
    Sent,
}

/// What a key press asks the model to do. The model owns every `Cmd` and every action.
pub(super) enum SendCmd {
    None,
    /// Run `validate_address` for this address, then build.
    Validate,
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
    /// The `@` picker: the address book and the selected row.
    pub picker: Option<(Vec<BookEntry>, usize)>,
    /// The form has the keys. `Esc` leaves it so that `1 2 3 4`, `q` and `?` work; `Enter`
    /// returns to it. Without this an address typed into the To field would switch screens.
    pub editing: bool,
}

/// The three fields, in Tab order, with the hint each draws while it is empty.
const FIELDS: [(Field, &str, &str); 3] = [
    (
        Field::To,
        "To",
        "an address, or @name from your address book",
    ),
    // The piconero form stays accepted and stays in `--help`; as the empty field's example it
    // taught a reader that amounts look like that (§2o rule 9).
    // **X22: an empty-field hint that cannot be read as a typed value.** `0.5 XMR` is exactly what
    // a real amount looks like, in the one field where reading the hint as the value costs money;
    // `To` and `Note` both read unmistakably as hints beside it.
    (Field::Amount, "Amount", "how much, in XMR"),
    (Field::Note, "Note", "optional, kept with this wallet"),
];

impl SendScreen {
    pub fn new(th: &Theme, priority: u32, account: u32) -> SendScreen {
        SendScreen {
            to: field_input(th),
            amount: field_input(th),
            note: field_input(th),
            field: Field::To,
            priority,
            account,
            stage: Stage::Form,
            typed: String::new(),
            picker: None,
            editing: false,
        }
    }

    pub fn restyle(&mut self, th: &Theme) {
        for input in [&mut self.to, &mut self.amount, &mut self.note] {
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

    pub fn back_to_form(&mut self) {
        self.stage = Stage::Form;
        self.typed.clear();
    }

    pub fn clear(&mut self) {
        self.to.clear();
        self.amount.clear();
        self.note.clear();
        self.field = Field::To;
        self.back_to_form();
    }

    /// The live validation line: `(text, ok)`. Each keystroke re-runs `address::parse` and
    /// `Piconero::parse` locally — nothing is sent (§7a step 1).
    pub fn validation(&self) -> (String, bool) {
        let to = self.to.value().trim();
        let amount = self.amount.value().trim();
        if amount.eq_ignore_ascii_case("all") {
            return (NO_SWEEP.to_string(), false);
        }
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
        match Piconero::parse(amount) {
            Ok(p) if p.0 == 0 => ("0 XMR is not an amount.".into(), false),
            // **M19: `the check` is named.** Three lines on this screen used the noun and none of
            // them said what it is — the wallet service works out the fee by preparing the payment
            // without sending it, which is the one thing a person needs to know before pressing a
            // key on a money screen.
            Ok(p) => (
                format!(
                    "{dest} · {} XMR · Enter works out the fee. Nothing is sent yet.",
                    p.to_xmr_string()
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
            max_fee,
        })
    }

    /// Focus the form field a click landed on (§7a.2), by its index in [`FIELDS`] order.
    pub fn focus_index(&mut self, index: usize) {
        let field = match index {
            0 => Field::To,
            1 => Field::Amount,
            2 => Field::Note,
            3 => Field::Priority,
            _ => Field::Account,
        };
        self.editing = true;
        self.focus(field);
    }

    /// The form, its picker and its confirm modal own every key while they are focused.
    /// While an action is in flight they own none, so `Esc` reaches the model and cancels it.
    pub fn owns_keys(&self) -> bool {
        self.confirming() || self.picker.is_some() || (self.editing && !self.busy())
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
                SendCmd::Info("You left the form. 1-4 change screens, Enter goes back.".into())
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
                    Field::Account => Field::To,
                });
                SendCmd::None
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus(match self.field {
                    Field::To => Field::Account,
                    Field::Amount => Field::To,
                    Field::Note => Field::Amount,
                    Field::Priority => Field::Note,
                    Field::Account => Field::Priority,
                });
                SendCmd::None
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
            code => {
                // `@` is typed AND opens the picker: a label is a legal recipient, so the
                // character stays if the picker is dismissed, and picking replaces the field
                // with a literal address (§4).
                let at = code == KeyCode::Char('@') && self.field == Field::To;
                self.focused_input_mut().handle_event(&Event::Key(k));
                if at {
                    SendCmd::Book
                } else {
                    SendCmd::None
                }
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
                self.typed.push(c);
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
    }

    pub fn focus(&mut self, f: Field) {
        self.to.set_focused(f == Field::To);
        self.amount.set_focused(f == Field::Amount);
        self.note.set_focused(f == Field::Note);
        self.field = f;
    }

    fn focused_input_mut(&mut self) -> &mut TextInput {
        match self.field {
            Field::Amount => &mut self.amount,
            Field::Note => &mut self.note,
            _ => &mut self.to,
        }
    }
}

fn field_input(th: &Theme) -> TextInput {
    TextInput::new()
        .with_style(th.text())
        .with_cursor_style(Style::new().bg(th.accent).fg(th.selection))
}

// ---------------------------------------------------------------- render

pub(super) fn view(m: &Tui, area: Rect, frame: &mut ftui::Frame, tier: Tier) {
    let th = &m.theme;
    let s = &m.send;
    if area.height < 4 {
        return;
    }
    // Wide: form on the left, a live preview on the right. Everything else: form only.
    let (form_area, preview) = if tier == Tier::Wide {
        let cols = Flex::horizontal()
            .constraints([Constraint::Percentage(58.0), Constraint::Fill])
            .split(area);
        (cols[0], Some(cols[1]))
    } else {
        (area, None)
    };

    let block = panel(th, " Send ", true);
    let inner = block.inner(form_area);
    block.render(form_area, frame);
    if inner.height < 3 {
        return;
    }
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(2),
            Constraint::Fixed(2),
            Constraint::Fixed(2),
            Constraint::Fixed(2),
            Constraint::Fill,
        ])
        .split(inner);
    for (i, (field, label, hint)) in FIELDS.into_iter().enumerate() {
        let row = rows[i];
        let input = match field {
            Field::Amount => &s.amount,
            Field::Note => &s.note,
            _ => &s.to,
        };
        Paragraph::new(Text::from_line(Line::from_spans([Span::styled(
            format!("{label:<7}"),
            if s.field == field {
                th.accented()
            } else {
                th.dimmed()
            },
        )])))
        .wrap(WrapMode::None)
        .render(row, frame);
        let box_ = Rect::new(row.x + 8, row.y, row.width.saturating_sub(8).max(1), 1);
        super::screens::hinted_input(th, input, hint, box_, frame);
        // **M17: a field whose visible value is not its value says so.** A 95-character address in
        // a 57-cell box shows its last 57 characters and the first 33 are off-screen with nothing
        // marking it — on the field that decides who gets the money. The row already owns two lines,
        // so the second one carries the fact. The Preview beside it still shows the address whole,
        // and this line is what a narrower terminal gets instead.
        let typed = input.value().width();
        if row.height > 1 && typed > box_.width as usize {
            Paragraph::new(Text::from_line(Line::styled(
                format!("        {typed} characters typed. The box is showing the end of them."),
                th.dimmed(),
            )))
            .wrap(WrapMode::None)
            .render(Rect::new(row.x, row.y + 1, row.width, 1), frame);
        }
        // §7a.2: a click focuses the field. The whole row is the target, label included, because
        // a user clicking "Amount" means the amount.
        m.hits.field(i, Rect::new(row.x, row.y, row.width, 1));
        let _ = box_;
    }
    // The priority chips carry the node's own `fees[]` for each level — no local multiplier
    // table (§1b.4); `0` is drawn as an alias of the lowest chip.
    let fees = m
        .status
        .as_ref()
        .and_then(|st| st.node.fee.fees_pico.clone())
        .unwrap_or_default();
    let mut chips = vec![Span::styled(
        "Fee".to_string(),
        if s.field == Field::Priority {
            th.accented()
        } else {
            th.dimmed()
        },
    )];
    // Each chip is **individually** clickable, and a click **selects that priority** rather than
    // cycling (§7a.2, revision 6) — which is also more discoverable than four presses of `p`.
    // Without this a mouse-only user could not change the fee on the money screen.
    // **Four chips, by name** (§2o): the old row was `Priority 0=1  1 20000p  2 80000p …` — an
    // alias notation and four per-byte piconero figures, none of which a person chooses between.
    // The node's own `fees[]` still decides what each level costs (§1b.4); it is simply not the
    // label. `0` is monerod's alias of the cheapest level, so it highlights that chip, and each
    // chip stays **individually** clickable (§7a.2, revision 6) — without that a mouse-only user
    // could not change the fee on the money screen.
    let selected = s.priority.max(1);
    let mut x = rows[3].x + "Fee".width() as u16;
    for (i, name) in super::FEE_LEVELS.iter().enumerate() {
        let level = i as u32 + 1;
        let _ = fees.get(i);
        let label = format!(" {name} ");
        let w = label.width() as u16;
        if x + w <= rows[3].right() {
            m.hits.priority(level, Rect::new(x, rows[3].y, w, 1));
        }
        x += w;
        chips.push(Span::styled(
            label,
            if level == selected {
                th.selected()
            } else {
                th.dimmed()
            },
        ));
    }
    chips.push(Span::styled(" p · Ctrl+P".to_string(), th.dimmed()));
    // The Account field lives on its OWN line. On one line with five fee chips the row ran past
    // the form's width at every tier, and what fell off the end was the focus highlight, the
    // value and the `←→` affordance for the field that decides which balance is spent.
    // `◂ n ▸` with **both arrows as hit rects** (§7a.2, revision 6): the account decides which
    // balance is spent, and a mouse-only user could not change it while it was `←→` only.
    let account = Line::from_spans([
        Span::styled(
            format!("Account ◂ {} ▸ ", s.account),
            if s.field == Field::Account {
                th.selected()
            } else {
                th.dimmed()
            },
        ),
        Span::styled("←→ changes it".to_string(), th.dimmed()),
    ]);
    if rows[3].height > 1 {
        let y = rows[3].y + 1;
        let lead = "Account ".width() as u16;
        m.hits.arrow(
            super::hit::Selector::Account,
            -1,
            Rect::new(rows[3].x + lead, y, 1, 1),
        );
        let value_w = format!("{}", s.account).width() as u16;
        m.hits.arrow(
            super::hit::Selector::Account,
            1,
            Rect::new(rows[3].x + lead + 2 + value_w + 1, y, 1, 1),
        );
    }
    Paragraph::new(Text::from_lines([Line::from_spans(chips), account]))
        .wrap(WrapMode::None)
        .render(rows[3], frame);

    let (text, ok) = s.validation();
    let (status, style) = match &s.stage {
        Stage::Validating => ("checking the address…".to_string(), th.warn()),
        Stage::Building => (
            "Checking the fee. Nothing has been sent yet. Esc cancels.".to_string(),
            th.warn(),
        ),
        Stage::Relaying => ("Sending. This cannot be stopped.".to_string(), th.warn()),
        // **No id at all now** (round 4, S10; X9 had shortened it, W8 had printed all 64
        // characters). An elided id is not a value: it cannot be pasted, filtered on or read
        // out. What the reader wants after a send is where the payment is, and it is one row
        // below on the screen this line sits at the bottom of.
        Stage::Sent => (
            "Sent. The payment is at the top of your payments.".to_string(),
            th.ok(),
        ),
        _ => (text, if ok { th.ok() } else { th.err() }),
    };
    // **The status line wraps too** (round 4's own read of the captures). It was rendered
    // `WrapMode::None` on one row, so `mainnet address · 0.0100 XMR · Enter works out the fee.
    // Nothing is sent yet.` was cut after `Nothing is` — on the money screen's own status line,
    // with the reassurance that matters the half that fell off. Its continuations take the same
    // hanging indent the footer below it does.
    let w = rows[4].width as usize;
    let text_w = w.saturating_sub(2).max(8);
    let mut lines: Vec<Line> = Vec::new();
    for (i, part) in super::screens::wrap_words(&status, text_w)
        .into_iter()
        .enumerate()
    {
        lines.push(Line::styled(
            if i == 0 { part } else { format!("  {part}") },
            style,
        ));
    }
    // **A wrapped key line keeps a hanging indent** (X15's shape, found by round 4's own read of the
    // captures): `WrapMode::Word` returned the continuation to column 0, so at 120 columns
    // `· Esc leaves` ended one row and `the form` started the next flush left, reading as a new
    // line of its own. The wrapping happens here and the paragraph is rendered unwrapped.
    let footer = format!(
        "account {} · ↑↓ fields · ⇥ screens · @ address book · {}",
        s.account,
        if s.editing {
            "Esc leaves the form"
        } else {
            "Enter returns to the form"
        }
    );
    for (i, part) in super::screens::wrap_words(&footer, text_w)
        .into_iter()
        .enumerate()
    {
        lines.push(Line::styled(
            if i == 0 { part } else { format!("  {part}") },
            th.dimmed(),
        ));
    }
    Paragraph::new(Text::from_lines(lines))
        .wrap(WrapMode::None)
        .render(rows[4], frame);

    if let Some(preview) = preview {
        view_preview(m, preview, frame);
    }
    if let Some((entries, sel)) = &s.picker {
        view_picker(m, entries, *sel, frame);
    }
}

/// The wide tier's live preview: what the request currently is, before anything is sent.
fn view_preview(m: &Tui, area: Rect, frame: &mut ftui::Frame) {
    let th = &m.theme;
    let block = panel(th, " Preview ", false);
    let inner = block.inner(area);
    block.render(area, frame);
    if inner.height == 0 {
        return;
    }
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
            // **Eight decimals once the fee is known** (round 4 iteration 3): with a plan in hand
            // this pane shows an Amount, a Fee and a Total, and `0.0100` above `0.00003072` above
            // `0.01003072` is the same three-precisions-in-one-column the confirm modal had. Before
            // the plan exists it is the figure the reader typed, which is theirs to read.
            match (amount, &s.stage) {
                (Some(a), Stage::Built(_)) => {
                    format!("{} XMR", a.to_xmr_min(Piconero::FEE_DECIMALS))
                }
                (Some(a), _) => format!("{} XMR", a.to_xmr_string()),
                (None, _) => "—".to_string(),
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
        // §2o.1: `wallet book add` is CLI-only, so the empty state states the fact and names
        // no command (docs/COPY.md §2).
        vec!["Your address book is empty.".into()]
    } else {
        entries
            .iter()
            .map(|e| format!("{} {}", fit(&e.description, 18), short_addr(&e.address)))
            .collect()
    };
    let sel = if entries.is_empty() { usize::MAX } else { sel };
    super::screens::list_modal(m, " Address book · Enter picks ", &rows, sel, None, frame);
}

/// The confirm modal — the whole safety surface, and therefore never abbreviated: it renders
/// in full at every tier (§7a's resize table).
pub(super) fn view_confirm(m: &Tui, plan: &Plan, frame: &mut ftui::Frame) {
    let th = &m.theme;
    // The modal is the whole safety surface and is never abbreviated (§7a's resize table), so
    // its height is computed from the width it will actually get: the recipient wraps into
    // more rows on a narrow terminal, and the typed-word prompt must still be on screen.
    let w = 78.min(frame.bounds().width);
    let addr_rows = wrapped(&plan.dest, w.saturating_sub(4) as usize).len() as u16;
    // The body is `addr_rows + 8` lines — "To", the wrapped address, Amount/Fee/Total, Account,
    // a blank, the typed-word prompt and the echo row — and `overlay`'s interior is `h - 4`
    // (a border row and a padding row at each end). `addr_rows + 11` was one short, and the row
    // that lost was the echo: the user typed the confirmation word into a modal that showed no
    // character of it, on the one screen that moves money (§7a: never abbreviated).
    let Some(inner) =
        super::screens::overlay(m, frame, w, addr_rows + 12, " Confirm · this sends money ")
    else {
        return;
    };
    let bg = th.selection;
    let dim = th.dimmed().bg(bg);
    let text = th.text().bg(bg);
    // §5's shape rule, on the screen §7a calls "the whole safety surface": unknown is `—`, never
    // a zero. `Piconero::checked_add` fails only when `amount + fee` exceeds `u64::MAX`, and
    // `Total 0.0000 XMR` beside a real Amount and a real Fee is not a sum of what is being spent.
    // The confirm word is refused in that state (`SendScreen::on_key`).
    // **All three figures at one precision** (round 4 iteration 3): `Amount 0.0100 XMR`,
    // `Fee 0.00003072 XMR`, `Total 0.01003072 XMR` — three different decimal counts in one modal,
    // on the three numbers the reader is being asked to add up. Eight decimals for all three, which
    // is what `Piconero::FEE_DECIMALS` is, and the sum is then checkable by eye down the column.
    let xmr = |p: Piconero| p.to_xmr_min(Piconero::FEE_DECIMALS);
    let (total, total_pico) = match plan.total() {
        Ok(t) => (xmr(t), t.0.to_string()),
        Err(_) => ("—".to_string(), "—".to_string()),
    };
    let mut lines = vec![Line::styled("To".to_string(), dim)];
    // Wrapped, never elided (§7a).
    lines.extend(
        wrapped(&plan.dest, inner.width as usize)
            .into_iter()
            .map(|l| Line::styled(l, th.accented().bg(bg))),
    );
    for (label, xmr, pico) in [
        (
            "Amount ",
            xmr(plan.total_amount),
            plan.total_amount.0.to_string(),
        ),
        ("Fee    ", xmr(plan.total_fee), plan.total_fee.0.to_string()),
        ("Total  ", total, total_pico),
    ] {
        lines.push(Line::from_spans([
            Span::styled(label.to_string(), dim),
            Span::styled(format!("{xmr} XMR  "), text),
            Span::styled(format!("({pico} piconero)"), dim),
        ]));
    }
    lines.push(Line::from_spans([
        Span::styled("Account".to_string(), dim),
        Span::styled(
            // The transaction's weight is an internal number and stays in `--json` (§2o).
            format!(
                " {}  Fee level {}",
                plan.account_index,
                super::fee_level_name(plan.priority)
            ),
            text,
        ),
    ]));
    lines.push(Line::styled(String::new(), dim));
    lines.push(Line::styled(
        if plan.total().is_err() {
            "That total is too large to add up, so it cannot be confirmed · Esc discards"
                .to_string()
        } else {
            format!("Type \"{CONFIRM_WORD}\" and press Enter to send · Esc discards")
        },
        th.warn().bg(bg),
    ));
    lines.push(Line::from_spans([
        Span::styled("> ".to_string(), th.accented().bg(bg)),
        Span::styled(m.send.typed.clone(), text),
        Span::styled("_".to_string(), dim),
    ]));
    Paragraph::new(Text::from_lines(lines))
        .wrap(WrapMode::None)
        .render(inner, frame);
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEST: &str = "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm";

    fn screen() -> SendScreen {
        SendScreen::new(&Theme::default(), 2, 0)
    }

    #[test]
    fn all_is_refused_inline_and_names_the_sweep_command() {
        let mut s = screen();
        s.to.set_value(DEST);
        s.amount.set_value("all");
        let (text, ok) = s.validation();
        assert!(!ok);
        assert_eq!(text, NO_SWEEP);
        assert!(text.contains("fmonero wallet sweep"));
        assert!(s.request(None).is_err());
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
