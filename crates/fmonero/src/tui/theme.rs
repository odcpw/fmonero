//! Leafline surface, ink and focus colours derived from the active Omarchy theme.
//! Node and wallet states keep their semantic colours. The receive QR uses dark ink on light paper.

use ftui::{PackedRgba, Style, TableTheme};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Theme {
    pub page: PackedRgba,
    pub panel: PackedRgba,
    pub fg: PackedRgba,
    pub dim: PackedRgba,
    pub accent: PackedRgba,
    rule: PackedRgba,
    selection: PackedRgba,
    pub red: PackedRgba,
    pub green: PackedRgba,
    pub yellow: PackedRgba,
}

/// A QR is only scannable as dark-on-light, whatever the terminal's own palette is, so the
/// two colours it needs are absolute rather than themed (§7a's QR modal).
const QR_LIGHT: PackedRgba = PackedRgba::rgb(0xff, 0xff, 0xff);
const QR_DARK: PackedRgba = PackedRgba::rgb(0x00, 0x00, 0x00);

impl Default for Theme {
    fn default() -> Self {
        Self::from_colors(&toml::Table::new(), false)
    }
}

pub fn colors_path() -> PathBuf {
    dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|dir| dir.join(".local/state")))
        .unwrap_or_default()
        .join("omarchy/current/theme/colors.toml")
}

pub fn parse_hex(s: &str) -> Option<PackedRgba> {
    let hex = s.trim().trim_start_matches('#').get(..6)?;
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(PackedRgba::rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

fn mix(a: PackedRgba, b: PackedRgba, percent: u16) -> PackedRgba {
    let channel = |a: u8, b: u8| ((a as u16 * (100 - percent) + b as u16 * percent) / 100) as u8;
    PackedRgba::rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

impl Theme {
    pub fn load() -> Self {
        let colors = std::fs::read_to_string(colors_path())
            .ok()
            .and_then(|s| s.parse::<toml::Table>().ok())
            .unwrap_or_default();
        Self::from_colors(&colors, std::env::var_os("NO_COLOR").is_some())
    }

    fn from_colors(colors: &toml::Table, monochrome: bool) -> Self {
        let get = |name: &str, fallback| {
            colors
                .get(name)
                .and_then(|v| v.as_str())
                .and_then(parse_hex)
                .unwrap_or(fallback)
        };
        let page = get("background", PackedRgba::rgb(0x11, 0x1a, 0x17));
        let fg = get("foreground", PackedRgba::rgb(0xe4, 0xe9, 0xdc));
        let accent = get("accent", PackedRgba::rgb(0xaf, 0xd2, 0xa4));
        let ink = |value| {
            if monochrome {
                PackedRgba::TRANSPARENT
            } else {
                value
            }
        };
        Self {
            page: ink(page),
            panel: ink(mix(page, fg, 6)),
            fg: ink(fg),
            dim: ink(mix(page, fg, 68)),
            accent: ink(accent),
            rule: ink(mix(page, fg, 27)),
            selection: ink(mix(page, accent, 24)),
            red: ink(get("red", PackedRgba::rgb(0xd4, 0x83, 0x83))),
            green: ink(get("green", PackedRgba::rgb(0xa0, 0xbd, 0x90))),
            yellow: ink(get("yellow", PackedRgba::rgb(0xda, 0xbd, 0x79))),
        }
    }

    fn ink(&self, color: PackedRgba) -> Style {
        if color == PackedRgba::TRANSPARENT {
            Style::new()
        } else {
            Style::new().fg(color)
        }
    }

    pub fn text(&self) -> Style {
        self.ink(self.fg)
    }
    pub fn dimmed(&self) -> Style {
        self.ink(self.dim)
    }
    pub fn accented(&self) -> Style {
        self.ink(self.accent).bold()
    }
    pub fn selected(&self) -> Style {
        if self.page == PackedRgba::TRANSPARENT {
            Style::new().bold().reverse()
        } else {
            self.text().bg(self.selection).bold()
        }
    }
    pub fn border(&self) -> Style {
        self.ink(self.rule)
    }
    pub fn border_focused(&self) -> Style {
        self.ink(self.accent)
    }
    pub fn title(&self) -> Style {
        self.text().bold()
    }
    pub fn ok(&self) -> Style {
        self.ink(self.green)
    }
    pub fn warn(&self) -> Style {
        self.ink(self.yellow)
    }
    pub fn err(&self) -> Style {
        self.ink(self.red)
    }
    pub fn key(&self) -> Style {
        self.accented()
    }
    pub fn table(&self) -> TableTheme {
        TableTheme::default()
            .with_border(self.border())
            .with_header(self.dimmed().bold())
            .with_row(self.text())
            .with_row_alt(self.text())
            .with_row_selected(self.selected())
            .with_row_hover(self.text())
            .with_divider(self.border())
    }
    pub fn cursor(&self) -> Style {
        self.text().reverse()
    }
    /// The §7a badge: green for synced / wallet open, yellow for syncing / wallet closed,
    /// dim for down, red for an error or an unresolved `pending_send`. Anything
    /// unrecognised is dim, never uncoloured.
    pub fn badge(&self, s: &str) -> Style {
        match s {
            "up" => self.ok().bold(),
            "syncing" => self.warn().bold(),
            "error" => self.err().bold(),
            _ => self.dimmed().bold(),
        }
    }

    /// The receive QR's style: **dark ink on light paper**, whatever the terminal's palette is.
    /// `fg` is the module colour and `bg` the quiet zone's, which is the way round a half-block
    /// canvas needs it — an unlit cell keeps the background, so the paper must be the background
    /// and the modules the thing that is drawn (see `screens::qr_modal`).
    pub fn qr(&self) -> Style {
        Style::new().fg(QR_DARK).bg(QR_LIGHT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_badge_state_has_its_own_colour() {
        let t = Theme::default();
        assert_eq!(t.badge("up").fg, Some(t.green));
        assert_eq!(t.badge("syncing").fg, Some(t.yellow));
        assert_eq!(t.badge("error").fg, Some(t.red));
        assert_eq!(t.badge("down").fg, Some(t.dim));
        assert_eq!(t.badge("nonsense").fg, Some(t.dim));
    }

    #[test]
    fn monochrome_keeps_selection_distinct_on_a_light_or_dark_terminal() {
        let theme = Theme::from_colors(&toml::Table::new(), true);
        assert_eq!(theme.text(), Style::new());
        assert_eq!(theme.selected(), Style::new().bold().reverse());
        assert_eq!(theme.cursor(), Style::new().reverse());
        assert_ne!(theme.qr().fg, theme.qr().bg);
    }

    #[test]
    fn the_qr_is_dark_on_light_whatever_the_theme_is() {
        let t = Theme::default();
        // Dark ink, light paper — the name of this test, and the polarity a decoder expects.
        assert_eq!(t.qr().fg, Some(QR_DARK));
        assert_eq!(t.qr().bg, Some(QR_LIGHT));
    }

    #[test]
    fn hex_parsing_tolerates_the_hash_and_trailing_alpha() {
        assert_eq!(parse_hex("#ff8000"), Some(PackedRgba::rgb(255, 128, 0)));
        assert_eq!(parse_hex("ff8000ff"), Some(PackedRgba::rgb(255, 128, 0)));
        assert_eq!(parse_hex("#fff"), None);
    }
}
