//! Protected forms for complete wallet workflows. Operations use typed core requests.
use fmonero_core::{restore, SecretString};
use ftui::layout::Rect;
use ftui::widgets::{input::TextInput, paragraph::Paragraph, Widget};
use ftui::{Cmd, Event, KeyCode, KeyEvent};

use super::{editor::SecretInput, hit::Button, screens, subs::Action, text_input, Msg, Tui};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Tool(super::advanced::Tool),
    Restore,
    CreateRequest,
    ParseRequest,
}

pub(super) enum Value {
    Public(Box<TextInput>),
    Secret(SecretInput),
}
pub(super) struct Field {
    pub label: &'static str,
    pub value: Value,
}
pub(super) struct Form {
    pub kind: Kind,
    pub identity: Option<fmonero_core::wallet_ops::WalletIdentity>,
    pub node_policy: Option<fmonero_core::settings::NodeSettings>,
    pub native_pending: Option<fmonero_core::native_tx::PendingSubmission>,
    pub review: bool,
    pub review_scroll: usize,
    pub review_max: std::cell::Cell<usize>,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
}

impl Form {
    pub fn restore(m: &Tui) -> Self {
        let fields = [
            ("Wallet name", false, ""),
            ("Seed words", true, ""),
            ("New password", true, ""),
            ("Password again", true, ""),
            ("Seed offset (optional)", true, ""),
            ("Restore height", false, "0"),
        ]
        .into_iter()
        .map(|(label, secret, initial)| Field {
            label,
            value: if secret {
                Value::Secret(SecretInput::new())
            } else {
                Value::Public(Box::new(text_input(&m.theme).with_value(initial)))
            },
        })
        .collect();
        let mut form = Self {
            kind: Kind::Restore,
            identity: None,
            node_policy: None,
            native_pending: None,
            review: false,
            review_scroll: 0,
            review_max: std::cell::Cell::new(0),
            fields,
            focus: 0,
            error: None,
        };
        form.focus(0);
        form
    }
    pub fn tool(
        m: &Tui,
        tool: super::advanced::Tool,
        identity: Option<fmonero_core::wallet_ops::WalletIdentity>,
    ) -> Self {
        let fields = tool
            .fields()
            .into_iter()
            .map(|(label, secret, initial)| Field {
                label,
                value: if secret {
                    Value::Secret(SecretInput::new())
                } else {
                    Value::Public(Box::new(text_input(&m.theme).with_value(initial)))
                },
            })
            .collect();
        let mut form = Self {
            kind: Kind::Tool(tool),
            identity,
            node_policy: (tool == super::advanced::Tool::Transport)
                .then(|| m.ctx.settings.node.clone()),
            native_pending: None,
            review: false,
            review_scroll: 0,
            review_max: std::cell::Cell::new(0),
            fields,
            focus: 0,
            error: None,
        };
        for field in &mut form.fields {
            if field.label == "Account" {
                if let Value::Public(input) = &mut field.value {
                    input.set_value(m.account_index().to_string());
                }
            }
        }
        if tool == super::advanced::Tool::Transport {
            let node = &m.ctx.settings.node;
            let values = [
                node.tls.to_string(),
                node.ca_file
                    .as_ref()
                    .map_or_else(String::new, |p| p.display().to_string()),
                node.fingerprint.clone().unwrap_or_default(),
                node.proxy.clone().unwrap_or_default(),
                node.remote_trusted.to_string(),
            ];
            for (field, value) in form.fields.iter_mut().zip(values) {
                if let Value::Public(input) = &mut field.value {
                    input.set_value(value);
                }
            }
        }
        form.focus(0);
        form
    }
    pub fn payment(m: &Tui, parse: bool) -> Self {
        let address = m.selected_address().unwrap_or_default();
        let specs = if parse {
            vec![("Monero payment URI", "")]
        } else {
            vec![
                ("Receiving address", address.as_str()),
                ("Amount XMR (optional)", ""),
                ("Description (optional)", ""),
                ("Recipient name (optional)", ""),
            ]
        };
        let mut form = Self {
            identity: None,
            node_policy: None,
            native_pending: None,
            review: false,
            review_scroll: 0,
            review_max: std::cell::Cell::new(0),
            kind: if parse {
                Kind::ParseRequest
            } else {
                Kind::CreateRequest
            },
            fields: specs
                .into_iter()
                .map(|(label, initial)| Field {
                    label,
                    value: Value::Public(Box::new(text_input(&m.theme).with_value(initial))),
                })
                .collect(),
            focus: 0,
            error: None,
        };
        form.focus(0);
        form
    }
    fn title(&self) -> &'static str {
        match self.kind {
            Kind::Tool(tool) => tool.title(),
            Kind::Restore => " Restore wallet ",
            Kind::CreateRequest => " Create payment request ",
            Kind::ParseRequest => " Open payment request ",
        }
    }
    fn submit_label(&self) -> &'static str {
        match self.kind {
            Kind::Tool(_) => {
                if self.review {
                    "Confirm operation"
                } else {
                    "Review operation"
                }
            }
            Kind::Restore => "Restore",
            _ => "Review request",
        }
    }
    pub fn focus(&mut self, index: usize) {
        self.focus = index.min(self.fields.len().saturating_sub(1));
        for (i, f) in self.fields.iter_mut().enumerate() {
            match &mut f.value {
                Value::Public(v) => v.set_focused(i == self.focus),
                Value::Secret(v) => v.set_focused(i == self.focus),
            }
        }
    }
    pub fn event(&mut self, event: &Event) {
        if self.review {
            return;
        }
        if let Some(f) = self.fields.get_mut(self.focus) {
            match &mut f.value {
                Value::Public(v) => {
                    v.handle_event(event);
                }
                Value::Secret(v) => {
                    v.handle_event(event);
                }
            }
        }
        self.error = self
            .fields
            .get(self.focus)
            .and_then(|field| match &field.value {
                Value::Secret(value) => value.error().map(str::to_string),
                _ => None,
            });
    }
    fn text(&self, i: usize) -> &str {
        match &self.fields[i].value {
            Value::Public(v) => v.value(),
            Value::Secret(v) => v.value(),
        }
    }
    fn secret(&mut self, i: usize) -> SecretString {
        match &mut self.fields[i].value {
            Value::Secret(v) => v.take(),
            _ => unreachable!("typed secret field"),
        }
    }
    fn advanced_action(&mut self, tool: super::advanced::Tool) -> Result<Action, String> {
        use super::advanced::Tool;
        if tool == Tool::ReopenNativePending {
            let expected = self
                .native_pending
                .clone()
                .ok_or("Reopen the pending-wallet inspection form.")?;
            return Ok(Action::Native(Box::new(
                super::native::Job::InspectPending {
                    expected,
                    password: self.secret(0),
                },
            )));
        }
        if matches!(tool, Tool::SignUnsigned | Tool::SubmitSigned) {
            let wallet = self
                .identity
                .clone()
                .ok_or("Open the intended wallet and reopen this form.")?;
            let source = std::path::PathBuf::from(self.text(0).trim());
            if !source.is_absolute() {
                return Err("Choose an absolute native artifact path.".into());
            }
            let signed = tool == Tool::SubmitSigned;
            let destination = if signed {
                None
            } else {
                let path = std::path::PathBuf::from(self.text(2).trim());
                if !path.is_absolute() {
                    return Err("Choose a new absolute signed-output path.".into());
                }
                Some(path)
            };
            let max_fee = if signed && !self.text(2).trim().is_empty() {
                Some(
                    fmonero_core::Piconero::parse(self.text(2).trim())
                        .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            return Ok(Action::Native(Box::new(super::native::Job::Begin {
                wallet,
                password: self.secret(1),
                source,
                destination,
                max_fee,
                signed,
            })));
        }
        self.advanced_job(tool).map(Action::Advanced)
    }
    fn advanced_job(
        &mut self,
        tool: super::advanced::Tool,
    ) -> Result<super::advanced::Job, String> {
        use super::advanced::{Job, Tool};
        match tool {
            Tool::Transport => Ok(Job::Transport {
                expected: self
                    .node_policy
                    .clone()
                    .ok_or("Reopen the transport form.")?,
                values: self
                    .fields
                    .iter()
                    .map(|field| match &field.value {
                        Value::Public(value) => value.value().to_string(),
                        Value::Secret(_) => String::new(),
                    })
                    .collect(),
            }),
            Tool::RecoveryBackup => {
                let wallet = self
                    .identity
                    .clone()
                    .ok_or("Open a wallet and reopen this form.")?;
                let path = std::path::PathBuf::from(self.text(0).trim());
                if !path.is_absolute() {
                    return Err("Choose a new absolute directory for the recovery wallet.".into());
                }
                if self.text(1).is_empty() || self.text(1) != self.text(2) {
                    return Err(
                        "Enter the recovery wallet password twice; both entries must match.".into(),
                    );
                }
                let height = self
                    .text(3)
                    .trim()
                    .parse()
                    .map_err(|_| "Restore height must be a whole number.")?;
                Ok(Job::RecoveryBackup {
                    wallet,
                    password: self.secret(1),
                    height,
                    path,
                })
            }
            Tool::CreateHardware => {
                let name = self.text(0).trim().to_string();
                if name.is_empty() || name.contains('/') || name.contains('\\') {
                    return Err("Choose a new wallet name without a path.".into());
                }
                let device = match self.text(1).trim() {
                    "ledger" => fmonero_core::hardware::Device::Ledger,
                    "trezor" => fmonero_core::hardware::Device::Trezor,
                    _ => return Err("Choose ledger or trezor.".into()),
                };
                if self.text(2).is_empty() || self.text(2) != self.text(3) {
                    return Err("Enter the wallet password twice; both entries must match.".into());
                }
                let restore_height = self
                    .text(4)
                    .trim()
                    .parse()
                    .map_err(|_| "Restore height must be a whole number.")?;
                Ok(Job::CreateHardware(
                    fmonero_core::hardware::CreateHardware {
                        name,
                        device,
                        password: self.secret(2),
                        restore_height,
                    },
                ))
            }
            Tool::ChangePassword => {
                let wallet = self
                    .identity
                    .clone()
                    .ok_or("Open the wallet and reopen this form.")?;
                if self.text(1).is_empty() {
                    return Err("Choose a nonempty new password.".into());
                }
                if self.text(1) != self.text(2) {
                    return Err("The new passwords do not match.".into());
                }
                Ok(Job::ChangePassword {
                    wallet,
                    old: self.secret(0),
                    new: self.secret(1),
                })
            }
            Tool::CreateViewOnly => {
                let name = self.text(0).trim().to_string();
                if name.is_empty() || name.contains('/') || name.contains('\\') {
                    return Err("Choose a new wallet name without a path.".into());
                }
                let address = self.text(1).trim().to_string();
                if self.text(2).len() != 64 || !self.text(2).bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(
                        "Enter a private view key of exactly 64 hexadecimal characters.".into(),
                    );
                }
                if self.text(3).is_empty() || self.text(3) != self.text(4) {
                    return Err("Enter a nonempty password twice; both entries must match.".into());
                }
                let restore_height = self
                    .text(5)
                    .trim()
                    .parse()
                    .map_err(|_| "Restore height must be a whole number.")?;
                Ok(Job::CreateViewOnly(fmonero_core::watch::CreateViewOnly {
                    name,
                    address,
                    view_key: self.secret(2),
                    password: self.secret(3),
                    restore_height,
                }))
            }
            _ => {
                let values = self
                    .fields
                    .iter()
                    .map(|f| match &f.value {
                        Value::Public(v) => v.value().to_string(),
                        Value::Secret(_) => String::new(),
                    })
                    .collect::<Vec<_>>();
                super::advanced::job(tool, self.identity.clone(), &values)
            }
        }
    }
    fn request(&mut self) -> Result<restore::RestoreRequest, String> {
        let name = self.text(0).trim().to_string();
        if name.is_empty() || name.contains('/') || name.contains('\\') {
            return Err("Choose a wallet name without a path.".into());
        }
        if self.text(1).trim().is_empty() {
            return Err("Enter the recovery seed words.".into());
        }
        if self.text(2).is_empty() {
            return Err("Choose a password to encrypt this wallet.".into());
        }
        if self.text(2) != self.text(3) {
            return Err("The passwords do not match.".into());
        }
        let restore_height = self.text(5).trim().parse().map_err(|_| {
            "Restore height must be a whole number. Use 0 to scan from the beginning."
        })?;
        let seed_offset = if self.text(4).is_empty() {
            None
        } else {
            Some(self.secret(4))
        };
        Ok(restore::RestoreRequest {
            name,
            seed: self.secret(1),
            password: self.secret(2),
            seed_offset,
            restore_height,
            language: None,
        })
    }
}

impl Tui {
    pub(super) fn open_tool(&mut self, tool: super::advanced::Tool) -> Cmd<Msg> {
        if tool == super::advanced::Tool::ReopenNativePending {
            match fmonero_core::native_tx::pending(&self.ctx) {
                Ok(Some(pending)) => {
                    let mut form = Form::tool(self, tool, Some(pending.wallet.clone()));
                    form.native_pending = Some(pending);
                    self.modal = Some(super::Modal::Workflow(form));
                }
                Ok(None) => self.note(
                    "No native submission needs recovery. Use Open on the Wallet page.",
                    false,
                ),
                Err(error) => self.note(error.to_string(), true),
            }
            return Cmd::none();
        }
        if tool == super::advanced::Tool::NativePending {
            return self.start(Action::Advanced(super::advanced::Job::NativePending));
        }
        if tool == super::advanced::Tool::Capabilities {
            return self.start(Action::Advanced(super::advanced::Job::Capabilities));
        }
        if tool.needs_identity() {
            self.start(Action::PrepareTool(tool))
        } else {
            self.modal = Some(super::Modal::Workflow(Form::tool(self, tool, None)));
            Cmd::none()
        }
    }
    pub(super) fn open_restore(&mut self) -> Cmd<Msg> {
        self.modal = Some(super::Modal::Workflow(Form::restore(self)));
        Cmd::none()
    }
    pub(super) fn open_request(&mut self, parse: bool) -> Cmd<Msg> {
        self.launch_intent = super::LaunchIntent::Default;
        self.modal = Some(super::Modal::Workflow(Form::payment(self, parse)));
        Cmd::none()
    }
    pub(super) fn on_tools_key(&mut self, k: KeyEvent) -> Cmd<Msg> {
        let items = tool_items(self.tools_group);
        match k.code {
            KeyCode::Left => {
                return self.click_button(Button::ToolGroup((self.tools_group + 3) % 4))
            }
            KeyCode::Right => {
                return self.click_button(Button::ToolGroup((self.tools_group + 1) % 4))
            }
            KeyCode::Down => {
                self.tools_selected = (self.tools_selected + 1).min(items.len().saturating_sub(1))
            }
            KeyCode::Up => self.tools_selected = self.tools_selected.saturating_sub(1),
            KeyCode::Enter => {
                if let Some((action, _, _)) = items.get(self.tools_selected) {
                    return self.click_button(*action);
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => return self.open_restore(),
            KeyCode::Char('o') | KeyCode::Char('O') => return self.open_wallet_modal(),
            KeyCode::Char('c') | KeyCode::Char('C') => return self.create_wallet_modal(),
            KeyCode::Char('a') | KeyCode::Char('A') => return self.click_button(Button::Accounts),
            _ => {}
        }
        Cmd::none()
    }
    pub(super) fn on_workflow_key(&mut self, mut form: Form, k: KeyEvent) -> Cmd<Msg> {
        match k.code {
            KeyCode::Down | KeyCode::PageDown if form.review => {
                form.review_scroll = form
                    .review_scroll
                    .saturating_add(1)
                    .min(form.review_max.get())
            }
            KeyCode::Up | KeyCode::PageUp if form.review => {
                form.review_scroll = form.review_scroll.saturating_sub(1)
            }
            KeyCode::Tab | KeyCode::BackTab if form.review => {}
            KeyCode::Escape if form.review => {
                form.review = false;
            }
            KeyCode::Escape => {
                if form.kind == Kind::Restore {
                    self.cancel_wallet_prerequisite();
                }
                return Cmd::none();
            }
            KeyCode::Tab | KeyCode::Down => form.focus((form.focus + 1) % form.fields.len()),
            KeyCode::BackTab | KeyCode::Up => {
                form.focus((form.focus + form.fields.len() - 1) % form.fields.len())
            }
            KeyCode::Enter if !form.review && form.focus + 1 < form.fields.len() => {
                form.focus(form.focus + 1)
            }
            KeyCode::Enter if matches!(form.kind, Kind::Tool(_)) => {
                if !form.review {
                    form.review = true;
                    form.review_scroll = 0;
                } else if let Kind::Tool(tool) = form.kind {
                    if self.action.is_some() {
                        form.error = Some("Wait for the current operation to finish.".into());
                    } else {
                        match form.advanced_action(tool) {
                            Ok(action) => return self.start(action),
                            Err(why) => {
                                form.error = Some(why);
                                form.review = false;
                            }
                        }
                    }
                }
            }
            KeyCode::Enter if form.kind == Kind::Restore => {
                if self.action.is_some() {
                    form.error =
                        Some("Wait for the current operation to finish before restoring.".into());
                } else {
                    match form.request() {
                        Ok(request) => return self.start(Action::Restore(request)),
                        Err(why) => form.error = Some(why),
                    }
                }
            }
            KeyCode::Enter => {
                let network = self.ctx.settings.node.network.net_type();
                let result = if form.kind == Kind::ParseRequest {
                    fmonero_core::payment_uri::parse(form.text(0).trim(), network)
                        .map_err(|e| e.to_string())
                } else {
                    (|| -> Result<fmonero_core::payment_uri::PaymentRequest, String> {
                        let amount = if form.text(1).trim().is_empty() {
                            None
                        } else {
                            Some(
                                fmonero_core::Piconero::parse(form.text(1).trim())
                                    .map_err(|e| e.to_string())?,
                            )
                        };
                        let optional = |i| {
                            (!form.text(i).trim().is_empty())
                                .then(|| form.text(i).trim().to_string())
                        };
                        Ok(fmonero_core::payment_uri::PaymentRequest {
                            address: form.text(0).trim().to_string(),
                            amount,
                            description: optional(2),
                            recipient_name: optional(3),
                        })
                    })()
                };
                match result.and_then(|request| {
                    fmonero_core::payment_uri::format(&request, network)
                        .map(|uri| (request, uri))
                        .map_err(|e| e.to_string())
                }) {
                    Ok((request, uri)) => {
                        if form.kind == Kind::CreateRequest {
                            self.receiving.request = request
                                .amount
                                .map(|amount| (request.address.clone(), amount));
                        }
                        self.modal = Some(super::Modal::PaymentRequest(super::requests::Review {
                            request,
                            uri,
                            for_payment: form.kind == Kind::ParseRequest,
                            scroll: 0,
                            max_scroll: std::cell::Cell::new(0),
                        }));
                        return Cmd::none();
                    }
                    Err(why) => form.error = Some(why),
                }
            }
            _ if !form.review => form.event(&Event::Key(k)),
            _ => {}
        }
        self.modal = Some(super::Modal::Workflow(form));
        Cmd::none()
    }
}

pub(super) fn tool_items(group: usize) -> Vec<(Button, &'static str, &'static str)> {
    use super::advanced::Tool;
    if group == 0 {
        vec![
            (
                Button::Restore,
                "Restore wallet",
                "Recover an encrypted wallet from protected seed input.",
            ),
            (
                Button::Accounts,
                "Accounts and contacts",
                "Create and select accounts; maintain verified recipients.",
            ),
            (
                Button::Tool(Tool::ExportHistory),
                "Export payment history",
                Tool::ExportHistory.description(),
            ),
            (
                Button::CreateRequest,
                "Create payment request",
                "Review and share a Monero URI with an optional exact amount.",
            ),
            (
                Button::ParseRequest,
                "Open payment request",
                "Check a URI before using it to fill the Send form.",
            ),
            (
                Button::Notifications,
                "Desktop payment alerts",
                "Opt in or out. New activity only; private generic notices.",
            ),
        ]
    } else if group == 1 {
        [
            Tool::Rescan,
            Tool::ChangePassword,
            Tool::Capabilities,
            Tool::Transport,
        ]
        .into_iter()
        .map(|tool| (Button::Tool(tool), tool.title(), tool.description()))
        .collect()
    } else if group == 2 {
        [
            Tool::RecoveryBackup,
            Tool::CreateHardware,
            Tool::PrepareUnsigned,
            Tool::InspectUnsigned,
            Tool::SignUnsigned,
            Tool::SubmitSigned,
            Tool::NativePending,
            Tool::ReopenNativePending,
            Tool::ClearNativePending,
            Tool::CreateViewOnly,
            Tool::ExportOutputs,
            Tool::ImportOutputs,
            Tool::ExportKeyImages,
            Tool::ImportKeyImages,
        ]
        .into_iter()
        .map(|tool| (Button::Tool(tool), tool.title(), tool.description()))
        .collect()
    } else {
        [
            Tool::SignMessage,
            Tool::VerifyMessage,
            Tool::TxProof,
            Tool::VerifyTxProof,
            Tool::SpendProof,
            Tool::VerifySpendProof,
            Tool::ReserveProof,
            Tool::VerifyReserveProof,
        ]
        .into_iter()
        .map(|tool| (Button::Tool(tool), tool.title(), tool.description()))
        .collect()
    }
}

pub(super) fn tools(m: &Tui, area: Rect, frame: &mut ftui::Frame) {
    if area.height < 3 {
        return;
    }
    let mut x = area.x;
    for (group, label) in ["Wallet", "Care", "Offline", "Proofs"]
        .into_iter()
        .enumerate()
    {
        let width = (label.len() as u16 + 3).min(area.right().saturating_sub(x));
        let rect = Rect::new(x, area.y, width, 1);
        Paragraph::new(format!(" {label} "))
            .style(if group == m.tools_group {
                m.theme.selected()
            } else {
                m.theme.key()
            })
            .render(rect, frame);
        m.hits.button(Button::ToolGroup(group), rect);
        x += width;
    }
    let items = tool_items(m.tools_group);
    let room = area.height.saturating_sub(3) as usize;
    let start = m.tools_selected.saturating_sub(room.saturating_sub(1));
    for (slot, (index, (action, title, description))) in
        items.iter().enumerate().skip(start).take(room).enumerate()
    {
        let row = Rect::new(area.x, area.y + 1 + slot as u16, area.width, 1);
        let title = if *action == Button::Notifications {
            format!(
                "{title}: {}",
                if m.ctx.settings.tui.notifications {
                    "on"
                } else {
                    "off"
                }
            )
        } else {
            title.to_string()
        };
        let text = if area.width >= 100 {
            format!(
                "{} {title:<29} {description}",
                if index == m.tools_selected {
                    "▌"
                } else {
                    " "
                }
            )
        } else {
            format!(
                "{} {title}",
                if index == m.tools_selected {
                    "▌"
                } else {
                    " "
                }
            )
        };
        Paragraph::new(text)
            .style(if index == m.tools_selected {
                m.theme.selected()
            } else {
                m.theme.text()
            })
            .render(row, frame);
        m.hits.button(*action, row);
    }
    let help = items
        .get(m.tools_selected)
        .map(|(_, _, description)| *description)
        .unwrap_or("←→ groups · ↑↓ actions · Enter opens");
    Paragraph::new(help).style(m.theme.dimmed()).render(
        Rect::new(area.x, area.bottom().saturating_sub(2), area.width, 1),
        frame,
    );
    screens::chip_row(
        m,
        Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        &[
            (Button::ToolsUp, "↑", "Previous"),
            (Button::ToolsDown, "↓", "More"),
        ],
        frame,
    );
}

pub(super) fn form(m: &Tui, form: &Form, frame: &mut ftui::Frame) {
    let Some(inner) = screens::overlay(m, frame, 76, 22, form.title()) else {
        return;
    };
    m.hits.modal(inner);
    if form.review {
        let mut lines = vec!["Review this operation. Esc returns to editing.".to_string()];
        if let Kind::Tool(tool) = form.kind {
            lines.push(tool.description().into());
            if matches!(
                tool,
                super::advanced::Tool::VerifyMessage
                    | super::advanced::Tool::VerifyTxProof
                    | super::advanced::Tool::VerifySpendProof
                    | super::advanced::Tool::VerifyReserveProof
            ) {
                lines.push("Confirmation uses Monero's public verification API and starts the owned wallet service if needed. No wallet selection or password is requested. Monero reports any further prerequisite.".into());
            }
        }
        if let Some(identity) = &form.identity {
            lines.push(if m.privacy() {
                "Wallet identity hidden by privacy.".into()
            } else {
                format!(
                    "Wallet: {}\nPrimary address: {}",
                    identity.name, identity.primary_address
                )
            });
        }
        if let Some(pending) = &form.native_pending {
            lines.push(if m.privacy() {
                "Native submission identity hidden by privacy. Its pending guard remains.".into()
            } else {
                format!("Artifact SHA-256 {}\nUnknown IDs {} · proven IDs {}\nThis record stays intact while you inspect history.", pending.artifact_sha256, pending.unknown_txids, pending.proven_txids.len())
            });
        }
        for field in &form.fields {
            let value = match &field.value {
                Value::Secret(v) => {
                    if v.value().is_empty() {
                        "(empty protected input)".into()
                    } else {
                        "Protected input · always masked".into()
                    }
                }
                Value::Public(_) if m.privacy() => "Hidden by privacy".into(),
                Value::Public(v) => v.value().to_string(),
            };
            lines.push(format!("{}\n{}", field.label, value));
        }
        let rows = lines
            .iter()
            .flat_map(|line| {
                line.lines()
                    .flat_map(|part| screens::wrapped(part, inner.width as usize))
            })
            .collect::<Vec<_>>();
        let room = inner.height.saturating_sub(2) as usize;
        form.review_max.set(rows.len().saturating_sub(room.max(1)));
        for (i, row) in rows
            .iter()
            .skip(form.review_scroll.min(form.review_max.get()))
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
        screens::modal_actions(m, inner, form.submit_label(), frame);
        return;
    }
    let room = (inner.height.saturating_sub(3) as usize / 2).max(1);
    let start = form.focus.saturating_sub(room.saturating_sub(1));
    for (slot, (i, field)) in form
        .fields
        .iter()
        .enumerate()
        .skip(start)
        .take(room)
        .enumerate()
    {
        let y = inner.y + (slot * 2) as u16;
        Paragraph::new(format!(
            "{}{}",
            if i == form.focus { "> " } else { "  " },
            field.label
        ))
        .style(m.theme.dimmed())
        .render(Rect::new(inner.x, y, inner.width, 1), frame);
        let area = Rect::new(inner.x + 2, y + 1, inner.width.saturating_sub(2), 1);
        match &field.value {
            Value::Secret(v) => v.masked_input(&m.theme).render(area, frame),
            Value::Public(v) if m.privacy() => {
                let mut masked = TextInput::new()
                    .with_value("•".repeat(v.value().chars().count()))
                    .with_style(m.theme.text());
                masked.set_focused(i == form.focus);
                masked.render(area, frame);
            }
            Value::Public(v) => v.render(area, frame),
        }
        if !form.review {
            m.hits.field(i, area);
        }
    }
    let hint = if form.error.is_some() && m.privacy() {
        "Input needs attention. Details hidden by privacy."
    } else {
        form.error.as_deref().unwrap_or(match form.kind {
            Kind::Tool(tool) => tool.description(),
            Kind::Restore if form.focus == 5 => "Use a block before the wallet was created. Only know its date? Use 0 for a complete, slower scan.",
            Kind::Restore => "Monero creates a new encrypted wallet. Existing wallets are never replaced.",
            _ => "Review before sharing or using this request. No payment is sent here.",
        })
    };
    Paragraph::new(hint)
        .wrap(ftui::text::wrap::WrapMode::Word)
        .style(if form.error.is_some() {
            m.theme.err()
        } else {
            m.theme.dimmed()
        })
        .render(
            Rect::new(inner.x, inner.bottom().saturating_sub(3), inner.width, 2),
            frame,
        );
    screens::modal_actions(m, inner, form.submit_label(), frame);
}
