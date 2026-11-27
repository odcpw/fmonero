//! Account and contact editing retains the exact identity and row reviewed by the user.
use super::{
    hit::{Button, HitPane},
    screens, text_input, Modal, Msg, Tui,
};
use fmonero_core::rpc::wallet::{AccountRow, BookEntry};
use fmonero_core::{accounts as core, wallet_ops::WalletIdentity, Context, Error, WalletClient};
use ftui::layout::Rect;
use ftui::widgets::{input::TextInput, paragraph::Paragraph, Widget};
use ftui::{Cmd, Event, Frame, KeyCode, KeyEvent};

pub(super) enum Job {
    Read,
    Create(WalletIdentity, String),
    Rename(WalletIdentity, AccountRow, String),
    Select(WalletIdentity, AccountRow),
    Add(WalletIdentity, String, String),
    Edit(WalletIdentity, BookEntry, String, String),
    Remove(WalletIdentity, BookEntry),
}
pub(super) struct Reply {
    pub message: String,
    pub snapshot: Option<core::Snapshot>,
    pub contacts: bool,
}
impl Job {
    pub fn execute(self, ctx: &Context) -> Result<Reply, Error> {
        let contacts = matches!(&self, Self::Add(..) | Self::Edit(..) | Self::Remove(..));
        let w = WalletClient::from_ctx(ctx)?;
        let message = match self {
            Self::Read => {
                return core::snapshot(ctx, &w).map(|snapshot| Reply {
                    message: String::new(),
                    snapshot: Some(snapshot),
                    contacts: false,
                })
            }
            Self::Create(identity, label) => {
                core::create(ctx, &w, &identity, &label)?;
                "Account created."
            }
            Self::Rename(identity, row, label) => {
                core::rename(ctx, &w, &identity, &row, &label)?;
                "Account label saved."
            }
            Self::Select(identity, row) => {
                core::select(ctx, &w, &identity, &row)?;
                "Account selected."
            }
            Self::Add(identity, address, label) => {
                core::add_contact(ctx, &w, &identity, &address, &label)?;
                "Contact added."
            }
            Self::Edit(identity, row, address, label) => {
                core::edit_contact(ctx, &w, &identity, &row, &address, &label)?;
                "Contact saved."
            }
            Self::Remove(identity, row) => {
                core::remove_contact(ctx, &w, &identity, &row)?;
                "Contact removed."
            }
        };
        // A failed refresh must not turn a successful mutation into an invitation to repeat it.
        let mut fresh = ctx.clone();
        if let Ok(settings) = fmonero_core::Settings::load(&ctx.paths) {
            fresh.settings = settings;
        }
        let snapshot = core::snapshot(&fresh, &w).ok();
        Ok(Reply {
            message: if snapshot.is_some() {
                message.into()
            } else {
                format!("{message} Reopen Accounts to refresh its rows.")
            },
            snapshot,
            contacts,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EditKind {
    CreateAccount,
    RenameAccount,
    AddContact,
    EditContact,
    RemoveContact,
}
pub(super) struct Panel {
    pub snapshot: core::Snapshot,
    pub contacts: bool,
    pub selected: usize,
}
pub(super) struct Editor {
    pub panel: Panel,
    pub kind: EditKind,
    pub fields: Vec<TextInput>,
    pub focus: usize,
    pub review: bool,
    pub scroll: usize,
    pub max_scroll: std::cell::Cell<usize>,
}

impl Panel {
    pub fn count(&self) -> usize {
        if self.contacts {
            self.snapshot.contacts.len()
        } else {
            self.snapshot.accounts.len()
        }
    }
}

impl Editor {
    fn new(m: &Tui, panel: Panel, kind: EditKind) -> Self {
        let values: Vec<String> = match kind {
            EditKind::CreateAccount => vec![String::new()],
            EditKind::RenameAccount => vec![panel
                .snapshot
                .accounts
                .get(panel.selected)
                .map(|a| a.label.clone())
                .unwrap_or_default()],
            EditKind::AddContact => vec![String::new(), String::new()],
            EditKind::EditContact => panel
                .snapshot
                .contacts
                .get(panel.selected)
                .map(|c| vec![c.address.clone(), c.description.clone()])
                .unwrap_or_default(),
            EditKind::RemoveContact => Vec::new(),
        };
        let fields = values
            .into_iter()
            .enumerate()
            .map(|(i, v)| text_input(&m.theme).with_value(v).with_focused(i == 0))
            .collect();
        Self {
            panel,
            kind,
            fields,
            focus: 0,
            review: kind == EditKind::RemoveContact,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        }
    }
    pub fn focus(&mut self, index: usize) {
        self.focus = index.min(self.fields.len().saturating_sub(1));
        for (i, field) in self.fields.iter_mut().enumerate() {
            field.set_focused(i == self.focus);
        }
    }
    pub fn event(&mut self, event: &Event) {
        if !self.review {
            if let Some(field) = self.fields.get_mut(self.focus) {
                field.handle_event(event);
            }
        }
    }
    fn job(&self) -> Option<Job> {
        let identity = self.panel.snapshot.wallet.clone();
        let value = |i: usize| {
            self.fields
                .get(i)
                .map(|f| f.value().trim().to_string())
                .unwrap_or_default()
        };
        Some(match self.kind {
            EditKind::CreateAccount => Job::Create(identity, value(0)),
            EditKind::RenameAccount => Job::Rename(
                identity,
                self.panel
                    .snapshot
                    .accounts
                    .get(self.panel.selected)?
                    .clone(),
                value(0),
            ),
            EditKind::AddContact => Job::Add(identity, value(0), value(1)),
            EditKind::EditContact => Job::Edit(
                identity,
                self.panel
                    .snapshot
                    .contacts
                    .get(self.panel.selected)?
                    .clone(),
                value(0),
                value(1),
            ),
            EditKind::RemoveContact => Job::Remove(
                identity,
                self.panel
                    .snapshot
                    .contacts
                    .get(self.panel.selected)?
                    .clone(),
            ),
        })
    }
}

impl Tui {
    pub(super) fn accounts_key(&mut self, mut panel: Panel, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Char('i') | KeyCode::Char('I') if panel.count() > 0 => {
                let lines = if panel.contacts {
                    let row = &panel.snapshot.contacts[panel.selected];
                    vec![
                        format!("Contact {}", row.index),
                        row.description.clone(),
                        row.address.clone(),
                    ]
                } else {
                    let row = &panel.snapshot.accounts[panel.selected];
                    vec![
                        format!("Account {}", row.account_index),
                        row.label.clone(),
                        row.base_address.clone(),
                        format!("Unlocked {} XMR", row.unlocked_balance.to_xmr_min(12)),
                    ]
                };
                self.modal_back = Some(Box::new(Modal::Accounts(panel)));
                self.modal = Some(Modal::Result(super::advanced::ResultView::complete(
                    "Account / contact details",
                    lines,
                )));
                return Cmd::none();
            }

            KeyCode::Escape => return Cmd::none(),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('b') | KeyCode::Char('B') => {
                panel.contacts = !panel.contacts;
                panel.selected = 0;
            }
            KeyCode::Down => {
                panel.selected = (panel.selected + 1).min(panel.count().saturating_sub(1))
            }
            KeyCode::Up => panel.selected = panel.selected.saturating_sub(1),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let kind = if panel.contacts {
                    EditKind::AddContact
                } else {
                    EditKind::CreateAccount
                };
                self.modal = Some(Modal::AccountEditor(Editor::new(self, panel, kind)));
                return Cmd::none();
            }
            KeyCode::Char('e') | KeyCode::Char('E') if panel.count() > 0 => {
                let kind = if panel.contacts {
                    EditKind::EditContact
                } else {
                    EditKind::RenameAccount
                };
                self.modal = Some(Modal::AccountEditor(Editor::new(self, panel, kind)));
                return Cmd::none();
            }
            KeyCode::Char('d') | KeyCode::Char('D') if panel.contacts && panel.count() > 0 => {
                self.modal = Some(Modal::AccountEditor(Editor::new(
                    self,
                    panel,
                    EditKind::RemoveContact,
                )));
                return Cmd::none();
            }
            KeyCode::Enter if panel.count() > 0 => {
                if panel.contacts {
                    self.send
                        .to
                        .set_value(&panel.snapshot.contacts[panel.selected].address);
                    self.send.back_to_form();
                    self.send.resume();
                    return self.set_screen(super::Screen::Send);
                }
                return self.start(super::subs::Action::Accounts(Job::Select(
                    panel.snapshot.wallet.clone(),
                    panel.snapshot.accounts[panel.selected].clone(),
                )));
            }
            _ => {}
        }
        self.modal = Some(Modal::Accounts(panel));
        Cmd::none()
    }
    pub(super) fn account_editor_key(&mut self, mut editor: Editor, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Down | KeyCode::PageDown if editor.review => {
                editor.scroll = editor.scroll.saturating_add(1).min(editor.max_scroll.get())
            }
            KeyCode::Up | KeyCode::PageUp if editor.review => {
                editor.scroll = editor.scroll.saturating_sub(1)
            }

            KeyCode::Escape if editor.review && editor.kind != EditKind::RemoveContact => {
                editor.review = false;
            }
            KeyCode::Escape => {
                self.modal = Some(Modal::Accounts(editor.panel));
                return Cmd::none();
            }
            KeyCode::Enter if editor.review => {
                if let Some(job) = editor.job() {
                    return self.start(super::subs::Action::Accounts(job));
                }
            }
            KeyCode::Enter => {
                editor.review = true;
            }
            KeyCode::Tab | KeyCode::Down if !editor.review => {
                editor.focus((editor.focus + 1) % editor.fields.len().max(1))
            }
            KeyCode::BackTab | KeyCode::Up if !editor.review => editor.focus(
                (editor.focus + editor.fields.len().saturating_sub(1)) % editor.fields.len().max(1),
            ),
            _ => editor.event(&Event::Key(k)),
        }
        self.modal = Some(Modal::AccountEditor(editor));
        Cmd::none()
    }
}

pub(super) fn view(m: &Tui, panel: &Panel, frame: &mut Frame) {
    let Some(inner) = screens::overlay(
        m,
        frame,
        86,
        frame.bounds().height.saturating_sub(1),
        if panel.contacts {
            " Contacts "
        } else {
            " Accounts "
        },
    ) else {
        return;
    };
    m.hits.modal(inner);
    let room = inner.height.saturating_sub(3) as usize;
    let start = panel.selected.saturating_sub(room.saturating_sub(1));
    for (slot, index) in (start..panel.count()).take(room).enumerate() {
        let value = if m.privacy() {
            format!("{} · details hidden", index + 1)
        } else if panel.contacts {
            let c = &panel.snapshot.contacts[index];
            format!("{}  {}  {}", c.index, c.description, c.address)
        } else {
            let a = &panel.snapshot.accounts[index];
            format!(
                "{}  {}  {} XMR",
                a.account_index,
                a.label,
                a.unlocked_balance.to_xmr_min(12)
            )
        };
        let row = Rect::new(inner.x, inner.y + slot as u16, inner.width, 1);
        Paragraph::new(format!(
            "{} {value}",
            if index == panel.selected { "▌" } else { " " }
        ))
        .style(if index == panel.selected {
            m.theme.selected()
        } else {
            m.theme.text()
        })
        .render(row, frame);
        m.hits.pane(HitPane::ModalList, row, row.y, 1, index);
    }
    if panel.count() == 0 {
        Paragraph::new(if panel.contacts {
            "No contacts. Add one with New."
        } else {
            "No accounts returned by the wallet."
        })
        .style(m.theme.dimmed())
        .render(inner, frame);
    }
    let mut buttons = vec![
        (
            Button::AccountSwitch,
            "B",
            if panel.contacts {
                "Accounts"
            } else {
                "Contacts"
            },
        ),
        (Button::AccountNew, "N", "New"),
    ];
    if panel.count() > 0 {
        buttons.push((Button::AccountEdit, "E", "Edit"));
        buttons.push((Button::AccountInfo, "I", "All info"));
        if panel.contacts {
            buttons.push((Button::AccountDelete, "D", "Delete"));
        }
    }
    screens::chip_row(
        m,
        Rect::new(inner.x, inner.bottom().saturating_sub(3), inner.width, 2),
        &buttons,
        frame,
    );
    screens::modal_actions(
        m,
        inner,
        if panel.contacts {
            "Use contact"
        } else {
            "Select account"
        },
        frame,
    );
}

pub(super) fn editor_view(m: &Tui, editor: &Editor, frame: &mut Frame) {
    let Some(inner) = screens::overlay(
        m,
        frame,
        80,
        18,
        if editor.review {
            " Confirm account/contact change "
        } else {
            " Edit account/contact "
        },
    ) else {
        return;
    };
    m.hits.modal(inner);
    if editor.review {
        let mut lines = vec!["Review the change. Esc returns without applying it.".to_string()];
        if editor.kind == EditKind::RemoveContact {
            lines.push("Remove this contact from the address book only.".into());
            if let Some(row) = editor.panel.snapshot.contacts.get(editor.panel.selected) {
                lines.push(if m.privacy() {
                    "Contact details hidden by privacy.".into()
                } else {
                    format!("{}\n{}", row.description, row.address)
                });
            }
        } else {
            for (i, field) in editor.fields.iter().enumerate() {
                let label = if editor.fields.len() == 1 {
                    "Label"
                } else if i == 0 {
                    "Address"
                } else {
                    "Contact name"
                };
                lines.push(format!(
                    "{label}\n{}",
                    if m.privacy() {
                        "Hidden by privacy"
                    } else {
                        field.value()
                    }
                ));
            }
        }
        let rows = lines
            .iter()
            .flat_map(|line| {
                line.lines()
                    .flat_map(|part| screens::wrapped(part, inner.width as usize))
            })
            .collect::<Vec<_>>();
        let room = inner.height.saturating_sub(2) as usize;
        editor
            .max_scroll
            .set(rows.len().saturating_sub(room.max(1)));
        for (i, row) in rows
            .iter()
            .skip(editor.scroll.min(editor.max_scroll.get()))
            .take(room)
            .enumerate()
        {
            Paragraph::new(row.as_str()).style(m.theme.text()).render(
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                frame,
            );
        }
        screens::chip_row(
            m,
            Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1),
            &[
                (Button::ResultUp, "↑", "Previous"),
                (Button::ResultDown, "↓", "More"),
            ],
            frame,
        );
    } else {
        let room = (inner.height.saturating_sub(1) as usize / 2).max(1);
        let start = editor.focus.saturating_sub(room - 1);
        for (slot, (i, input)) in editor
            .fields
            .iter()
            .enumerate()
            .skip(start)
            .take(room)
            .enumerate()
        {
            let label = if editor.fields.len() == 1 {
                "Label"
            } else if i == 0 {
                "Address"
            } else {
                "Contact name"
            };
            let y = inner.y + slot as u16 * 2;
            Paragraph::new(label)
                .style(m.theme.dimmed())
                .render(Rect::new(inner.x, y, inner.width, 1), frame);
            let area = Rect::new(inner.x, y + 1, inner.width, 1);
            if m.privacy() {
                TextInput::new()
                    .with_value("•".repeat(input.value().chars().count()))
                    .render(area, frame);
            } else {
                input.render(area, frame);
            }
            m.hits.field(i, area);
        }
    }
    screens::modal_actions(
        m,
        inner,
        if editor.review {
            "Confirm change"
        } else {
            "Review change"
        },
        frame,
    );
}
