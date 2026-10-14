//! Secret editing never places plaintext in a widget's cloneable String storage.
use fmonero_core::SecretString;
use ftui::widgets::input::TextInput;
use ftui::{Event, KeyCode, Modifiers};

use super::theme::Theme;

pub(super) struct SecretInput {
    value: SecretString,
    cursor: usize,
    focused: bool,
    error: Option<&'static str>,
}

impl SecretInput {
    pub fn new() -> Self {
        Self {
            value: SecretString::empty(),
            cursor: 0,
            focused: false,
            error: None,
        }
    }
    pub fn value(&self) -> &str {
        self.value.expose_secret()
    }
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub fn error(&self) -> Option<&'static str> {
        self.error
    }
    pub fn clear(&mut self) {
        self.error = None;
        self.value.clear();
        self.cursor = 0;
    }
    pub fn take(&mut self) -> SecretString {
        self.error = None;
        self.cursor = 0;
        std::mem::take(&mut self.value)
    }
    fn previous(&self) -> usize {
        self.value()
            .get(..self.cursor)
            .and_then(|s| s.char_indices().next_back())
            .map_or(0, |(i, _)| i)
    }
    fn next(&self) -> usize {
        self.value()[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }
    pub fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }
        self.error = None;
        match event {
            Event::Paste(paste) => {
                // Paste is a single bounded edit. Newlines never dispatch form actions.
                if self.value.insert_str(self.cursor, &paste.text).is_ok() {
                    self.cursor += paste.text.len();
                } else {
                    self.error = Some("Secret input exceeds its bound or contains a NUL. The edit was not applied.");
                }
            }
            Event::Key(k) => match k.code {
                KeyCode::Left => self.cursor = self.previous(),
                KeyCode::Right => self.cursor = self.next(),
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = self.value.len(),
                KeyCode::Backspace if self.cursor > 0 => {
                    self.cursor = self.previous();
                    let _ = self.value.remove(self.cursor);
                }
                KeyCode::Delete if self.cursor < self.value.len() => {
                    let _ = self.value.remove(self.cursor);
                }
                KeyCode::Char('u') if k.modifiers.contains(Modifiers::CTRL) => self.clear(),
                KeyCode::Char(c)
                    if !k.modifiers.intersects(Modifiers::CTRL | Modifiers::ALT)
                        && !c.is_control() =>
                {
                    let mut bytes = [0; 4];
                    if self
                        .value
                        .insert_str(self.cursor, c.encode_utf8(&mut bytes))
                        .is_ok()
                    {
                        self.cursor += c.len_utf8();
                    } else {
                        self.error = Some("Secret input is full. The edit was not applied.");
                    }
                    zeroize::Zeroize::zeroize(&mut bytes);
                }
                _ => return false,
            },
            _ => return false,
        }
        true
    }
    pub fn masked_input(&self, theme: &Theme) -> TextInput {
        let mut input = TextInput::new()
            .with_value("•".repeat(self.value.char_count()))
            .with_style(theme.text());
        input.set_focused(true);
        for _ in self.value()[self.cursor..].chars() {
            input.handle_event(&Event::Key(ftui::KeyEvent::new(KeyCode::Left)));
        }
        input.set_focused(self.focused);
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ftui::KeyEvent;

    #[test]
    fn rejected_secret_edit_is_atomic_and_reports_its_bound() {
        let mut field = SecretInput::new();
        field.set_focused(true);
        field
            .value
            .push_str(&"x".repeat(fmonero_core::secrets::MAX_SECRET_BYTES))
            .unwrap();
        field.cursor = field.value.len();
        field.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('é'))));
        assert_eq!(field.value.len(), fmonero_core::secrets::MAX_SECRET_BYTES);
        assert!(field.error().is_some());
        field.handle_event(&Event::Key(KeyEvent::new(KeyCode::Backspace)));
        assert_eq!(
            field.value.len(),
            fmonero_core::secrets::MAX_SECRET_BYTES - 1
        );
        assert!(field.error().is_none());
        field.clear();
        assert!(field.value.is_empty());
    }

    #[test]
    fn secret_editor_edits_utf8_without_plaintext_widget_storage() {
        let mut field = SecretInput::new();
        field.set_focused(true);
        for ch in ['a', 'é', '界'] {
            field.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char(ch))));
        }
        field.handle_event(&Event::Key(KeyEvent::new(KeyCode::Left)));
        field.handle_event(&Event::Key(KeyEvent::new(KeyCode::Backspace)));
        assert_eq!(field.value(), "a界");
        assert_eq!(field.masked_input(&Theme::default()).value(), "••");
        let moved = field.take();
        assert!(field.value().is_empty());
        assert_eq!(moved.expose_secret(), "a界");
    }
}
