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

use fmonero_core::rpc::wallet::BookEntry;
use fmonero_core::{address, send::Plan, Piconero, SendRequest};

use super::screens::{fit, panel, short_addr, wrapped, Tier};
use super::theme::Theme;
use super::Tui;

/// Typing this word is the TUI's `--yes` (§7a): the modal needs the word, not just Enter.
pub(super) const CONFIRM_WORD: &str = "send";

/// What the Amount field says when someone types `all` — the same sentence §4 gives the CLI.
pub(super) const NO_SWEEP: &str =
    "sweep is not available in the TUI — run: fmonero wallet sweep <addr> --dry-run";

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
    Sent(String),
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
    (Field::To, "To", "address, or @label from the address book"),
    (Field::Amount, "Amount", "0.5   ·   250000000000p"),
    (Field::Note, "Note", "optional note, kept in the wallet"),
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
            return ("enter a recipient and an amount".into(), false);
        }
        let dest = if let Some(label) = to.strip_prefix('@') {
            if label.is_empty() {
                return ("@ needs an address-book label after it".into(), false);
            }
            format!("address book: {label}")
        } else {
            match address::parse(to) {
                Ok(a) => format!("{} {}", a.net, a.kind),
                Err(e) => return (e.to_string(), false),
            }
        };
        match Piconero::parse(amount) {
            Ok(p) if p.0 == 0 => ("0 is not an amount to send".into(), false),
            Ok(p) => (
                format!("{dest} · {} XMR · Enter builds it", p.to_xmr_string()),
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
        if let Stage::Sent(_) = self.stage {
            self.back_to_form();
        }
        let ctrl = k.modifiers.contains(ftui::Modifiers::CTRL);
        match k.code {
            KeyCode::Escape => {
                self.editing = false;
                self.blur();
                SendCmd::Info("form unfocused — 1..4 switch screens, Enter returns".into())
            }
            KeyCode::Char('p') if ctrl => {
                self.priority = (self.priority + 1) % 5;
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
                    KeyCode::Char('p') | KeyCode::Right => self.priority = (self.priority + 1) % 5,
                    KeyCode::Left => self.priority = (self.priority + 4) % 5,
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
                SendCmd::Note("discarded — nothing was broadcast".into())
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
                SendCmd::Note("this plan's total overflows u64 — Esc discards it".into())
            }
            KeyCode::Enter if self.typed.trim() == CONFIRM_WORD => SendCmd::Relay,
            KeyCode::Enter => SendCmd::Note(format!("type \"{CONFIRM_WORD}\" to broadcast")),
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
        super::screens::hinted_input(
            th,
            input,
            hint,
            Rect::new(row.x + 8, row.y, row.width.saturating_sub(8).max(1), 1),
            frame,
        );
    }
    // The priority chips carry the node's own `fees[]` for each level — no local multiplier
    // table (§1b.4); `0` is drawn as an alias of the lowest chip.
    let fees = m
        .status
        .as_ref()
        .and_then(|st| st.node.fee.fees_pico.clone())
        .unwrap_or_default();
    let mut chips = vec![Span::styled(
        "Priority".to_string(),
        if s.field == Field::Priority {
            th.accented()
        } else {
            th.dimmed()
        },
    )];
    for level in 0..5u32 {
        let fee = fees.get(level.saturating_sub(1).min(3) as usize);
        let label = match (level, fee) {
            (0, _) => " 0=1 ".to_string(),
            (l, Some(f)) => format!(" {l} {} ", f.to_xmr_string()),
            (l, None) => format!(" {l} — "),
        };
        chips.push(Span::styled(
            label,
            if level == s.priority {
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
    let account = Line::from_spans([
        Span::styled(
            format!("Account {} ", s.account),
            if s.field == Field::Account {
                th.selected()
            } else {
                th.dimmed()
            },
        ),
        Span::styled("←→ changes it".to_string(), th.dimmed()),
    ]);
    Paragraph::new(Text::from_lines([Line::from_spans(chips), account]))
        .wrap(WrapMode::None)
        .render(rows[3], frame);

    let (text, ok) = s.validation();
    let line = match &s.stage {
        Stage::Validating => Line::styled("checking the address…".to_string(), th.warn()),
        Stage::Building => Line::styled(
            "building with do_not_relay — nothing is broadcast yet · Esc cancels".to_string(),
            th.warn(),
        ),
        Stage::Relaying => Line::styled(
            "broadcasting — this one cannot be cancelled".to_string(),
            th.warn(),
        ),
        Stage::Sent(txid) => Line::styled(format!("sent · {txid}"), th.ok()),
        _ => Line::styled(text, if ok { th.ok() } else { th.err() }),
    };
    let mut lines = vec![line];
    lines.push(Line::styled(
        format!(
            "account {} · Tab moves · @ picks from the address book · {}",
            s.account,
            if s.editing {
                "Esc leaves the form"
            } else {
                "Enter returns to the form"
            }
        ),
        th.dimmed(),
    ));
    Paragraph::new(Text::from_lines(lines))
        .wrap(WrapMode::Word)
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
    // The recipient is never elided: an elided address is how you send to the wrong one.
    lines.extend(
        wrapped(s.to.value(), inner.width as usize)
            .into_iter()
            .map(|l| Line::styled(l, th.text())),
    );
    let amount = Piconero::parse(s.amount.value().trim()).ok();
    lines.push(Line::from_spans([
        Span::styled("Amount  ".to_string(), th.dimmed()),
        Span::styled(
            amount.map_or("—".to_string(), |a| format!("{} XMR", a.to_xmr_string())),
            th.text(),
        ),
    ]));
    lines.push(Line::from_spans([
        Span::styled("Fee     ".to_string(), th.dimmed()),
        Span::styled("known only after the build".to_string(), th.dimmed()),
    ]));
    lines.push(Line::from_spans([
        Span::styled("Account ".to_string(), th.dimmed()),
        Span::styled(s.account.to_string(), th.text()),
    ]));
    if let Some(w) = m.status.as_ref().map(|st| &st.wallet) {
        lines.push(Line::from_spans([
            Span::styled("Unlocked".to_string(), th.dimmed()),
            Span::styled(
                format!(" {}", w.unlocked_xmr.clone().unwrap_or_else(|| "—".into())),
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
        vec!["the book is empty — fmonero wallet book add <address> <description>".into()]
    } else {
        entries
            .iter()
            .map(|e| format!("{} {}", fit(&e.description, 18), short_addr(&e.address)))
            .collect()
    };
    let sel = if entries.is_empty() { usize::MAX } else { sel };
    super::screens::list_modal(m, " Address book — Enter picks ", &rows, sel, None, frame);
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
    let Some(inner) = super::screens::overlay(
        m,
        frame,
        w,
        addr_rows + 12,
        " Confirm — this broadcasts a transaction ",
    ) else {
        return;
    };
    let bg = th.selection;
    let dim = th.dimmed().bg(bg);
    let text = th.text().bg(bg);
    // §5's shape rule, on the screen §7a calls "the whole safety surface": unknown is `—`, never
    // a zero. `Piconero::checked_add` fails only when `amount + fee` exceeds `u64::MAX`, and
    // `Total 0.0000 XMR` beside a real Amount and a real Fee is not a sum of what is being spent.
    // The confirm word is refused in that state (`SendScreen::on_key`).
    let (total, total_pico) = match plan.total() {
        Ok(t) => (t.to_xmr_string(), t.0.to_string()),
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
            plan.total_amount.to_xmr_string(),
            plan.total_amount.0.to_string(),
        ),
        (
            "Fee    ",
            plan.total_fee.to_xmr_string(),
            plan.total_fee.0.to_string(),
        ),
        ("Total  ", total, total_pico),
    ] {
        lines.push(Line::from_spans([
            Span::styled(label.to_string(), dim),
            Span::styled(format!("{xmr} XMR  "), text),
            Span::styled(format!("({pico} pico)"), dim),
        ]));
    }
    lines.push(Line::from_spans([
        Span::styled("Account".to_string(), dim),
        Span::styled(
            format!(
                " {}  Priority {}  Weight {}",
                plan.account_index, plan.priority, plan.total_weight
            ),
            text,
        ),
    ]));
    lines.push(Line::styled(String::new(), dim));
    lines.push(Line::styled(
        if plan.total().is_err() {
            "this plan's total overflows u64 and cannot be confirmed · Esc to discard".to_string()
        } else {
            format!("Type \"{CONFIRM_WORD}\" and press Enter to broadcast · Esc to discard")
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
        assert_eq!(s.validation().0, "0 is not an amount to send");
        s.amount.set_value("0.5");
        s.to.set_value("not-an-address");
        assert!(!s.validation().1);
        s.to.set_value("@");
        assert!(s.validation().0.contains("label"));
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
            Some("enter a recipient and an amount".to_string()),
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
