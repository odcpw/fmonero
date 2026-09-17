//! Colours from the active Omarchy theme (`~/.local/state/omarchy/current/theme/colors.toml`),
//! with a neutral fallback when not on Omarchy.
//!
//! The loader is copied verbatim from `fproton/crates/fproton/src/tui/theme.rs`
//! (DESIGN.md §7a) and carries two fmonero-specific styles: [`Theme::badge`] — the §7a
//! status badge — and [`Theme::qr`], the receive QR's own black-on-white. **No literal
//! colour appears anywhere else in `src/tui/**`** (§7a); this file is the exception that
//! makes the rule checkable.

use ftui::{PackedRgba, Style};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Theme {
    pub fg: PackedRgba,
    pub dim: PackedRgba,
    pub accent: PackedRgba,
    pub selection: PackedRgba,
    pub muted: PackedRgba,
    pub red: PackedRgba,
    pub green: PackedRgba,
    pub yellow: PackedRgba,
    pub blue: PackedRgba,
    pub cyan: PackedRgba,
    pub magenta: PackedRgba,
    pub name: String,
}

/// A QR is only scannable as dark-on-light, whatever the terminal's own palette is, so the
/// two colours it needs are absolute rather than themed (§7a's QR modal).
const QR_LIGHT: PackedRgba = PackedRgba::rgb(0xff, 0xff, 0xff);
const QR_DARK: PackedRgba = PackedRgba::rgb(0x00, 0x00, 0x00);

impl Default for Theme {
    fn default() -> Self {
        Self {
            fg: PackedRgba::rgb(0xca, 0xcc, 0xcc),
            dim: PackedRgba::rgb(0x70, 0x78, 0x80),
            accent: PackedRgba::rgb(0xca, 0xcc, 0xcc),
            selection: PackedRgba::rgb(0x2a, 0x2e, 0x33),
            muted: PackedRgba::rgb(0x70, 0x78, 0x80),
            red: PackedRgba::rgb(0xa5, 0x55, 0x55),
            green: PackedRgba::rgb(0x8a, 0xa8, 0x72),
            yellow: PackedRgba::rgb(0xc9, 0xa5, 0x54),
            blue: PackedRgba::rgb(0x6c, 0x8e, 0xbf),
            cyan: PackedRgba::rgb(0x6f, 0xa8, 0xa8),
            magenta: PackedRgba::rgb(0xa0, 0x7a, 0xb0),
            name: "default".into(),
        }
    }
}

pub fn colors_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".local/state/omarchy/current/theme/colors.toml")
}

pub fn parse_hex(s: &str) -> Option<PackedRgba> {
    let h = s.trim().trim_start_matches('#');
    if h.len() < 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(PackedRgba::rgb(r, g, b))
}

impl Theme {
    pub fn load() -> Self {
        let mut t = Theme::default();
        let Ok(raw) = std::fs::read_to_string(colors_path()) else {
            return t;
        };
        let Ok(v) = raw.parse::<toml::Table>() else {
            return t;
        };
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(parse_hex);
        if let Some(c) = get("foreground") {
            t.fg = c;
        }
        if let Some(c) = get("accent") {
            t.accent = c;
        }
        if let Some(c) = get("selection") {
            t.selection = c;
        }
        if let Some(c) = get("muted") {
            t.muted = c;
            t.dim = c;
        }
        if let Some(c) = get("dark_foreground") {
            t.dim = c;
        }
        if let Some(c) = get("red") {
            t.red = c;
        }
        if let Some(c) = get("green") {
            t.green = c;
        }
        if let Some(c) = get("yellow") {
            t.yellow = c;
        }
        if let Some(c) = get("blue") {
            t.blue = c;
        }
        if let Some(c) = get("cyan") {
            t.cyan = c;
        }
        if let Some(c) = get("magenta") {
            t.magenta = c;
        }
        t.name = std::fs::read_link(colors_path().parent().unwrap_or(&PathBuf::from("/")))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "omarchy".into());
        t
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.fg)
    }
    pub fn dimmed(&self) -> Style {
        Style::new().fg(self.dim)
    }
    pub fn accented(&self) -> Style {
        Style::new().fg(self.accent).bold()
    }
    pub fn selected(&self) -> Style {
        Style::new().bg(self.selection).fg(self.fg).bold()
    }
    pub fn border(&self) -> Style {
        Style::new().fg(self.dim)
    }
    pub fn border_focused(&self) -> Style {
        Style::new().fg(self.accent)
    }
    pub fn title(&self) -> Style {
        Style::new().fg(self.fg).bold()
    }
    pub fn ok(&self) -> Style {
        Style::new().fg(self.green)
    }
    pub fn warn(&self) -> Style {
        Style::new().fg(self.yellow)
    }
    pub fn err(&self) -> Style {
        Style::new().fg(self.red)
    }
    pub fn key(&self) -> Style {
        Style::new().fg(self.accent).bold()
    }

    /// The §7a badge: green for synced / wallet open, yellow for syncing / wallet closed,
    /// dim for down, red for an error or an unresolved `pending_send`. Anything
    /// unrecognised is dim, never uncoloured.
    pub fn badge(&self, s: &str) -> Style {
        match s {
            "up" => Style::new().fg(self.green).bold(),
            "syncing" => Style::new().fg(self.yellow).bold(),
            "error" => Style::new().fg(self.red).bold(),
            _ => Style::new().fg(self.dim).bold(),
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
