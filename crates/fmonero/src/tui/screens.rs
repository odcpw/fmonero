//! Every cell the TUI draws (DESIGN.md §7a): the chrome, the four screens, the modals, the
//! help overlay and the four resize tiers. `view` returns on a zero area and never leaves a
//! screen blank — each empty, error and hang state has its own text.

use ftui::layout::{Constraint, Flex, Rect, Sides};
use ftui::text::wrap::WrapMode;
use ftui::text::{Line, Span, Text};
use ftui::widgets::block::Block;
use ftui::widgets::borders::BorderType;
use ftui::widgets::input::TextInput;
use ftui::widgets::paragraph::Paragraph;
use ftui::widgets::progress::{MiniBar, MiniBarColors};
use ftui::widgets::sparkline::Sparkline;
use ftui::widgets::table::{Row as TableRow, Table};
use ftui::widgets::virtualized::{RenderItem, VirtualizedList};
use ftui::widgets::{StatefulWidget, StatusItem, StatusLine, Tab, Tabs, Widget};
use ftui::{Frame, Style};
use ftui_extras::canvas::{Canvas, Mode, Painter};
use qrcode::{Color as QrColor, QrCode};
use unicode_width::UnicodeWidthStr;

use fmonero_core::{history, units::Layout, Severity, Status, Transfer};

use super::hit::{Button, HitPane, Selector};
use super::theme::Theme;
use super::{Modal, NodeView, Pane, Screen, Tui, FEE_LEVELS, KEYS, SPINNER};

/// Which layout the terminal has room for (§7a's resize table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tier {
    /// ≥ 100 columns: two columns, sparkline, live send preview.
    Wide,
    /// 60–99: one column, tables lose their least important columns, no sparkline.
    Medium,
    /// < 60 columns or < 20 rows: status line, hero and the one focused list.
    Narrow,
    /// < 40×10: one honest line.
    Tiny,
}

pub(super) fn tier_of(b: Rect) -> Tier {
    if b.width < 40 || b.height < 10 {
        Tier::Tiny
    } else if b.width < 60 || b.height < 20 {
        Tier::Narrow
    } else if b.width >= 100 {
        Tier::Wide
    } else {
        Tier::Medium
    }
}

pub(super) fn view(m: &Tui, frame: &mut Frame) {
    let b = frame.bounds();
    // §7a.2: the hit map is the layout of **this** frame and nothing else. A screen that stops
    // drawing a chip must stop accepting clicks on it, so the map is cleared here and rebuilt by
    // the same code that draws.
    m.hits.begin();
    if b.width == 0 || b.height == 0 {
        return;
    }
    let th = &m.theme;
    let tier = tier_of(b);
    if tier == Tier::Tiny {
        para(
            "Window too small. Make it at least 40 by 10.",
            th.warn(),
            true,
        )
        .render(Rect::new(b.x, b.y, b.width, b.height.min(3)), frame);
        return;
    }
    // The banner carries the txid AND both commands in full (§7a), which is about 135 columns of
    // text — so its height is the wrap of its own text at this width, measured with the same
    // helper that draws it. A hand-written ladder dropped the `clear-pending` half at 45 columns,
    // and that is the only command that gets the Send screen building again.
    let banner = m
        .status
        .as_ref()
        .and_then(|s| s.pending.as_ref())
        .map_or(0, |p| {
            wrap_words(&pending_banner_text(p), b.width as usize).len() as u16
        });
    // **W14: a toast that does not fit the status bar gets a row of its own**, directly above it
    // and the full width of the window. The bar's left half carries two badges, the node's
    // address and the balance, so what is left for a toast is between 18 and 44 columns — and the
    // refusal the whole of §2o.1 is built on is 40. It was cut to `Press f for the…` at both
    // sizes, which named neither of the two routes it exists to name. Shortening a sentence until
    // it fits the worst case is how it stops saying anything; the room is the fix.
    // **M11: a toast does not survive into a modal.** The row above the status bar still read
    // `Select a node first, with ↑↓ or a click, then Enter.` while the ` Use this node ` confirm
    // for the node that HAD been selected was open on top of it — an instruction contradicted by
    // the window over it. A modal states its own situation, which is what makes the toast beneath
    // it stale by definition. [`toast_hidden`] is the one predicate, so the status bar's copy and
    // this row cannot disagree.
    let toast_lines: Vec<String> = if toast_hidden(m) || note_fits(m, b.width) {
        Vec::new()
    } else {
        // Two rows at most: a toast is a sentence, and a third row of one is a panel.
        let mut lines = wrap_words(&m.note, b.width as usize);
        lines.truncate(2);
        lines
    };
    let toast = toast_lines.len() as u16;
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(1),
            Constraint::Fixed(banner),
            Constraint::Fill,
            Constraint::Fixed(toast),
            Constraint::Fixed(1),
        ])
        .split(b);
    tab_strip(m, rows[0], frame, tier);
    if banner > 0 {
        pending_banner(m, rows[1], frame);
    }
    if toast > 0 {
        let style = if m.note_err { th.err() } else { th.ok() };
        lines_at(
            toast_lines
                .into_iter()
                .map(|l| Line::styled(l, style))
                .collect(),
            rows[3],
            false,
            frame,
        );
    }
    match m.screen {
        Screen::Node => node_screen(m, rows[2], frame, tier),
        Screen::Wallet => wallet_screen(m, rows[2], frame, tier),
        Screen::Send => super::send::view(m, rows[2], frame, tier),
        Screen::Log => log_screen(m, rows[2], frame),
    }
    status_line(m, rows[4], frame);
    match &m.modal {
        // **V4: `Use your own node` is an overlay, not a one-row `Dialog`.** §7a.4 asks it to state
        // three things and `Dialog` draws its message as a single centred row, so a `\n` in it
        // spilled past the border and the two sentences after the first were unreadable. The keys
        // are unchanged — `y` confirms, anything else cancels, and a click on the button is `Enter`
        // (§7a.2) — because only the drawing was wrong.
        Some(Modal::Confirm {
            what: super::Confirm::NodeLocal,
            ..
        }) => local_node_modal(m, frame),
        // W2: `u` and `d` state what they do, in the shape `l` got in v5.6.1. `Dialog` draws its
        // message as one centred row, which is why these two were a title and a keys line.
        Some(Modal::Confirm {
            what: super::Confirm::NodeUp,
            ..
        }) => node_power_modal(m, true, frame),
        Some(Modal::Confirm {
            what: super::Confirm::NodeDown,
            ..
        }) => node_power_modal(m, false, frame),
        // **X4: the last confirm `Dialog` still drew, and it was cut mid-word at every width.**
        // `Dialog::confirm`'s box is a fixed 58 columns and three of the five messages are 60
        // characters, so `y to confirm · Esc cance` was on screen at 120x32 and at 150x40 alike,
        // with six blank rows under the cut — V15's lesson exactly. An overlay, like the three
        // above it.
        Some(Modal::Confirm {
            what: super::Confirm::LogLevel(level),
            ..
        }) => log_level_modal(m, *level, frame),
        // **No generic `Confirm` arm any more** (X4): all four confirms are overlays, because
        // `Dialog` draws its message as one centred row in a fixed 58-column box and every one of
        // them has more than one sentence to state. The `Dialog` value is still built — the key
        // handler and the mouse's `ModalButton` share its state — and it is no longer drawn.
        Some(Modal::OpenWallet {
            names,
            sel,
            pass,
            asking,
        }) => {
            let rows: Vec<String> = if let Some(err) = m.wallets_err.as_deref() {
                // **S1: fmonero could not read the folder, and it must not say the folder is
                // empty.** This modal said *"There are no wallets yet, in this folder:"* over a
                // path holding a wallet, and then named the key that creates a new one. The
                // sentence core built is the honest one, and the next step is the folder, not a
                // create.
                vec![
                    err.to_string(),
                    m.ctx.settings.wallet.dir.display().to_string(),
                    "Press o again once you can read that folder.".into(),
                ]
            } else if names.is_empty() {
                // §2o.1: one key creates a wallet, so the empty state names the key, not the
                // command it would have printed into a modal nobody can type into.
                //
                // **X16: the folder on its own line.** `No wallets in /tmp/…/data/fmonero/w` was
                // cut at both sizes with no marker, and the part lost was the folder's own name —
                // the only thing in the sentence a reader could act on. `list_modal` wraps a row
                // now; splitting the sentence in two also means the path starts at a known column.
                vec![
                    "There are no wallets yet, in this folder:".into(),
                    m.ctx.settings.wallet.dir.display().to_string(),
                    "Press c to create one.".into(),
                ]
            } else {
                names.clone()
            };
            list_modal(
                m,
                // W18: the keys are stated once, by `list_modal` itself. This title also said
                // them, differently (`Enter opens` against the body's `Enter chooses`), and its
                // em-dash is one §2o rule 12 forbids.
                " Open a wallet ",
                &rows,
                if names.is_empty() { usize::MAX } else { *sel },
                asking.then_some((pass, "password ")),
                frame,
            );
        }
        // **M5: the field has a label.** It was an unlabelled row under the keys line, so the one
        // thing in the modal a person types into was the one thing with no name on it. `Prompt`
        // carries its own word now, because "note" and "name" are different fields.
        Some(Modal::Prompt {
            title,
            input,
            field,
            ..
        }) => {
            list_modal(
                m,
                title,
                &[],
                usize::MAX,
                Some((input, field.as_str())),
                frame,
            );
        }
        Some(Modal::Qr { address }) => qr_modal(m, address, frame),
        Some(Modal::CreateWallet(form)) => create_modal(m, form, frame),
        Some(Modal::SeedBackup { lines }) => seed_modal(m, lines, frame),
        Some(Modal::UseNode {
            address,
            wallet_open,
        }) => use_node_modal(m, address, wallet_open.as_deref(), frame),
        Some(Modal::AddNode { input }) => add_node_modal(m, input, frame),
        Some(Modal::Notice { .. }) => notice_modal(m, frame),
        None => {}
    }
    if let super::send::Stage::Built(plan) = &m.send.stage {
        super::send::view_confirm(m, plan, frame);
    }
    if m.wallet.detail {
        detail_modal(m, frame);
    }
    if m.show_help {
        // Any click closes it (§7a.2 priority 1): an overlay that covers the screen and ignores
        // clicks is a screen the user cannot leave with the device that opened it.
        m.hits.overlay();
        help(m, frame);
    }
}

// ---------------------------------------------------------------- chrome

fn tab_strip(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    let th = &m.theme;
    if tier == Tier::Narrow {
        let mut spans = Vec::new();
        let mut x = area.x;
        for (i, label) in ["1", "2", "3", "4"].iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("·", th.dimmed()));
                x += 1;
            }
            let style = if i == m.screen.index() {
                th.accented()
            } else {
                th.dimmed()
            };
            spans.push(Span::styled((*label).to_string(), style));
            // Even collapsed to `1·2·3·4`, each digit is a hit target: the tier where a user is
            // most likely to be stuck is not the tier to take the way out away (§7a resize).
            m.hits.tab(i, Rect::new(x, area.y, 1, 1));
            x += 1;
        }
        spans.push(Span::styled(format!("  {}", m.screen.name()), th.title()));
        line_at(spans, area, frame);
        return;
    }
    let cols = Flex::horizontal()
        .constraints([Constraint::Fill, Constraint::FitContent])
        .split(area);
    let tabs = Tabs::new([
        Tab::new(" 1 Node "),
        Tab::new(" 2 Wallet "),
        Tab::new(" 3 Send "),
        Tab::new(" 4 Log "),
    ])
    .style(th.dimmed())
    .active_style(th.selected())
    .separator("");
    StatefulWidget::render(&tabs, cols[0], frame, &mut m.tabs.borrow_mut());
    // The fix for "the tabs look clickable and are not" (§7a.2), from the geometry `Tabs`
    // **draws**: `tab_label` wraps every tab in one leading and one trailing cell — `[`/`]` when
    // active, a space each side when not (`ftui-widgets-0.7.0/src/tabs.rs:239-256`) — so a tab is
    // `title.width() + 2` wide, with `.separator("")` costing nothing between (`:474-479`).
    // Advancing by the bare title width put every rect 2 columns narrow and `2 × index` columns
    // left of its label: `Send` selected `Log`, `4 Log` nothing (DECISIONS: DEFECT A). The labels
    // total 41 columns against this tier's 60-column floor, so the overflow markers (`:445-462`)
    // cannot fire; the `min` is the clip `draw_text_span` applies at the pane edge, and a tab with
    // no room left registers a zero-width rect, which `Map::at` can never match.
    let mut x = cols[0].x;
    for (i, label) in [" 1 Node ", " 2 Wallet ", " 3 Send ", " 4 Log "]
        .iter()
        .enumerate()
    {
        let w = (label.width() as u16 + 2).min(cols[0].right().saturating_sub(x));
        m.hits.tab(i, Rect::new(x, cols[0].y, w, 1));
        x += w;
    }

    let mut right = String::new();
    if m.poll_inflight || m.action.is_some() {
        right.push_str(SPINNER[m.tick % SPINNER.len()]);
        right.push(' ');
    }
    // **M16: the corner said `mainnet omarchy`** — two bare words with no label, the second of
    // them the name of the active colour theme, which is not a fact about anybody's money. The
    // network is worth a corner because sending real XMR from a testnet wallet is the mistake it
    // prevents, so it keeps its place and gains the word that says what it is. The theme's name
    // is in `fmonero settings get tui.theme` for whoever wants it.
    if let Some(net) = m.status.as_ref().and_then(|s| s.node.nettype.clone()) {
        right.push_str(&format!("network {net}"));
    }
    let w = (right.width() as u16 + 1).min(area.width);
    para(right, th.dimmed(), false).render(
        Rect::new(area.x + area.width.saturating_sub(w), area.y, w, 1),
        frame,
    );
}

/// The §7a banner's text, in one place so that `view` can measure what `pending_banner` draws.
///
/// **The two shell commands are gone** (§2o.1): this banner's whole job is telling the user how
/// to find out whether their money moved, and it used to do that by printing a command line into
/// a panel nobody can type into. The Wallet screen — one key away, and it is named — is where
/// the payment and its confirmations are.
fn pending_banner_text(p: &fmonero_core::PendingStatus) -> String {
    // **No id here** (round 4, S10). It was elided to fifteen characters, which is a value a
    // reader can neither paste, nor type into the Payments filter, nor read out to anyone — it
    // added alarm and nothing else to the one banner that is already alarming. The amount, the
    // destination and the counter are the three facts he can recognise, and the last clause is
    // what to do next. Every character of the id is in `--json`.
    format!(
        "SENDING {} XMR to {} · {} of {} sent · press 2 for the wallet",
        p.total_amount_xmr,
        short_addr(&p.dest),
        p.relayed_count,
        p.total_count
    )
}

/// The §7a red banner: on **every** screen while a `pending.json` record is unresolved.
fn pending_banner(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(p) = m.status.as_ref().and_then(|s| s.pending.as_ref()) else {
        return;
    };
    let lines = wrap_words(&pending_banner_text(p), area.width as usize)
        .into_iter()
        .map(|l| Line::styled(l, th.err()))
        .collect();
    lines_at(lines, area, false, frame);
}

/// The status bar's left half: the two badges and what is known beside them (§7a.1).
///
/// Its own function because **its width decides where the toast goes** (W14): a sentence that does
/// not fit beside these spans is drawn on a row of its own, and the only honest way to know that
/// is to measure the spans rather than guess at them.
fn status_left(m: &Tui) -> Vec<Span<'static>> {
    let th = &m.theme;
    let (node, wallet) = (m.node_badge(), m.wallet_badge());
    let (node_tone, wallet_tone) = (m.node_tone(), m.wallet_tone());
    // **With a public node the segment names the node, not a row label** (§2o, v5.6): the
    // badge word already is the noun, so ` node public node ` was a stutter, and the address
    // is the one thing a user who has just switched needs to read back (§7a.7).
    let remote = m.remote_mode();
    let mut spans = vec![Span::styled("●", th.badge(node_tone))];
    if remote {
        let addr = m
            .status
            .as_ref()
            .map(|s| s.node.url.clone())
            .unwrap_or_default();
        spans.push(Span::styled(format!(" {node} {addr} "), th.text()));
    } else {
        spans.push(Span::styled(format!(" node {node} "), th.text()));
    }
    if let Some(s) = &m.status {
        // **A separator between the two figures** (round 4's own read of the captures): the bar read
        // `block 3 541 122 16 out · 3 in`, where `3 541 122 16` is one number until you work out
        // that it is two facts. `·` is the field separator this program uses everywhere else.
        let mut fields: Vec<String> = Vec::new();
        if let Some(h) = s.node.height {
            // W7: `node synced 3541122` sat one row under a pane reading `block 3 541 122`.
            fields.push(format!("block {}", grouped(h)));
        }
        if let Some(out) = s.node.outgoing_connections_count {
            // The hero's own words, not `16↑3↓`: a reader takes those arrows for traffic
            // (self-read, round 3). §2o rule 8, one vocabulary.
            let inc = s.node.incoming_connections_count.unwrap_or(0);
            fields.push(format!("{out} out · {inc} in"));
        }
        if !fields.is_empty() {
            spans.push(Span::styled(
                format!("{} ", fields.join(" · ")),
                th.dimmed(),
            ));
        }
        spans.push(Span::styled("●", th.badge(wallet_tone)));
        spans.push(Span::styled(format!(" wallet {wallet} "), th.text()));
        if let Some(b) = &s.wallet.balance_xmr {
            spans.push(Span::styled(format!("{b} XMR "), th.accented()));
        }
    }
    if let Some(a) = &m.action {
        let tail = if a.cancellable { " · Esc cancels" } else { "" };
        spans.push(Span::styled("│ ", th.dimmed()));
        spans.push(Span::styled(
            format!(
                "{} {} {}s / {}s{tail} ",
                SPINNER[m.tick % SPINNER.len()],
                a.label,
                a.started.elapsed().as_secs(),
                a.deadline_sec
            ),
            th.warn(),
        ));
    }
    spans
}

fn status_left_width(m: &Tui) -> usize {
    status_left(m).iter().map(|s| s.content.width()).sum()
}

/// **X12: one key, one label, on every pane of the Node screen.** `f` is `start_fetch` wherever it
/// is pressed here, and it was labelled `Choose another node` in the ` This node ` pane and
/// `Get the list` in the ` Public nodes ` pane — two chips with the same key and different
/// meanings, in one frame. Choosing a node is `Enter` on a row, and that chip says so.
fn list_chip(m: &Tui) -> (Button, &'static str, &'static str) {
    (
        Button::RefreshList,
        "f",
        if m.picker.list.is_some() {
            "Get a fresh list"
        } else {
            "Get the list"
        },
    )
}

/// Is the toast suppressed altogether (M11)?
///
/// While a modal — or the send confirm, or the help overlay — is up, the window on top states the
/// situation and the toast underneath it answers a key press the reader has already moved past.
fn toast_hidden(m: &Tui) -> bool {
    m.note.is_empty()
        || m.modal.is_some()
        || m.show_help
        || m.wallet.detail
        || matches!(m.send.stage, super::send::Stage::Built(_))
}

/// Does the whole toast fit on the status bar beside the badges (W14)?
///
/// The same arithmetic `status_line` lays out with: `19` is the width of the `⇥ screens  ? help`
/// segment on the right, and the left segment is measured rather than estimated — its width
/// depends on the node's address, the balance and whether an action is running.
fn note_fits(m: &Tui, width: u16) -> bool {
    let avail = (width.saturating_sub(19) as usize).saturating_sub(status_left_width(m) + 2);
    m.note.width() <= avail
}

fn status_line(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    // **`⇥ screens` is §7a.1's rule, stated on every screen and in every focus state** (v5.5).
    // The rule's whole value is that it is true everywhere, and a user who has just been trapped
    // in a field needs to read it from where they are. Both segments are hit targets, because
    // they are the two things on this line already labelled like controls (§7a.2).
    let right = "⇥ screens  ? help";
    let rw = right.width() as u16;
    if area.width > rw {
        let x = area.right() - rw;
        para(right.to_string(), th.dimmed(), false).render(Rect::new(x, area.y, rw, 1), frame);
        m.hits.status(
            Rect::new(x + rw - 6, area.y, 6, 1),
            Rect::new(x, area.y, 9, 1),
        );
    } else {
        StatusLine::new()
            .style(th.dimmed())
            .right(StatusItem::key_hint("?", "help"))
            .render(area, frame);
    }
    let mut spans = status_left(m);
    // **V15: the toast is cut with an ellipsis, at a word boundary.** This line is rendered
    // `WrapMode::None` into the width left of `⇥ screens  ? help`, so the terminal used to cut the
    // last span wherever it happened to land: `A wallet is open. Press x to close i` is a complete
    // sentence about a key that does not exist.
    let avail = area.width.saturating_sub(19) as usize;
    // W14: only when the whole sentence fits. When it does not, `view` has given it a row of its
    // own above this one, and printing a cut copy here as well would be the same sentence twice,
    // one of them false.
    if !toast_hidden(m) && note_fits(m, area.width) {
        spans.push(Span::styled("│ ", th.dimmed()));
        spans.push(Span::styled(
            m.note.clone(),
            if m.note_err { th.err() } else { th.ok() },
        ));
    }
    line_at(spans, Rect::new(area.x, area.y, avail as u16, 1), frame);
}

// ---------------------------------------------------------------- Node

fn node_screen(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    if area.height < 4 {
        return;
    }
    // **v5.5: the picker is on this screen in both modes** (§7a.3). In remote mode it is the main
    // pane and fills the space the peers table occupies locally — every pane that needs a gated
    // method is unavailable there, which was four fifths of the owner's dead end. In local mode it
    // sits below the peers table and is how someone finds an address to switch to.
    let remote = m.status.as_ref().map(|s| s.node.layout) == Some(Layout::Remote);
    match tier {
        Tier::Wide => {
            // The peers table needs its own full width for the pruning seed and the rates
            // (§7a), so it sits under the two panel columns rather than inside one of them.
            let rows = Flex::vertical()
                .constraints([Constraint::Fixed(11), Constraint::Fill])
                .split(area);
            let top = Flex::horizontal()
                .constraints([Constraint::Percentage(52.0), Constraint::Fill])
                .split(rows[0]);
            node_hero(m, top[0], frame);
            // **With a public node the two panes that said "not available" are gone** and one
            // pane of what is known stands in their place (§7a.3, v5.6).
            if remote {
                this_node_pane(m, top[1], frame);
            } else {
                let right = Flex::vertical()
                    .constraints([Constraint::Fixed(5), Constraint::Fill])
                    .split(top[1]);
                net_pane(m, right[0], frame, true);
                pool_pane(m, right[1], frame);
            }
            let bottom = Flex::horizontal()
                .constraints([Constraint::Percentage(62.0), Constraint::Fill])
                .split(rows[1]);
            warnings_pane(m, bottom[1], frame);
            if remote {
                picker_pane(m, bottom[0], frame, tier);
            } else {
                let left = Flex::vertical()
                    .constraints([Constraint::Percentage(45.0), Constraint::Fill])
                    .split(bottom[0]);
                list_pane(m, left[0], frame, left[0].width >= 70);
                picker_pane(m, left[1], frame, tier);
            }
        }
        Tier::Medium => {
            // The panes DEGRADE in priority order instead of overflowing. Fixed(10) + Fixed(6)
            // + Fill + Fixed(5) needs 24 rows, and this tier starts at 20 (18 for the screen):
            // the peers table and the Warnings pane were both handed less than `framed`'s
            // 3-row minimum and drew nothing at all, so a 70x20 terminal showed two blank rows
            // where the only `severity: error` surface on this screen should be (§7a: "Empty,
            // error and hang states — each must render, none may be blank"). Warnings are never
            // the pane that goes.
            let h = area.height;
            if h < 18 {
                let rows = Flex::vertical()
                    .constraints([Constraint::Fill, Constraint::Fixed(5)])
                    .split(area);
                node_hero(m, rows[0], frame);
                warnings_pane(m, rows[1], frame);
            } else if h < 24 {
                let rows = Flex::vertical()
                    .constraints([
                        Constraint::Fixed(10),
                        Constraint::Fill,
                        Constraint::Fixed(5),
                    ])
                    .split(area);
                node_hero(m, rows[0], frame);
                if remote {
                    picker_pane(m, rows[1], frame, tier);
                } else {
                    list_pane(m, rows[1], frame, false);
                }
                warnings_pane(m, rows[2], frame);
            } else {
                let rows = Flex::vertical()
                    .constraints([
                        Constraint::Fixed(10),
                        Constraint::Fill,
                        Constraint::Fill,
                        Constraint::Fixed(5),
                    ])
                    .split(area);
                node_hero(m, rows[0], frame);
                if remote {
                    this_node_pane(m, rows[1], frame);
                } else {
                    list_pane(m, rows[1], frame, false);
                }
                picker_pane(m, rows[2], frame, tier);
                warnings_pane(m, rows[3], frame);
            }
        }
        _ => {
            // Narrow keeps the hero, the picker (address, health and — when probed — `ms`) and
            // the warnings pane. **Warnings are never the pane that goes** (§7a), and the picker
            // stays because the tier where a user is most likely to be stuck is not the tier to
            // hide the way out.
            let rows = Flex::vertical()
                .constraints([Constraint::Fixed(9), Constraint::Fill, Constraint::Fixed(4)])
                .split(area);
            node_hero(m, rows[0], frame);
            picker_pane(m, rows[1], frame, tier);
            warnings_pane(m, rows[2], frame);
        }
    }
}

/// §2n.4's machine reason, as a person reads it (§2o rule 5). The reason string itself is the
/// discriminator `node list --json` carries and agents branch on; this is the cell.
pub(super) use fmonero_core::nodes::probe_reason_words as probe_reason;

/// The public-node picker (§7a.4, v5.5) — the Node screen's lower pane, in both modes.
///
/// It answers the owner's *"fetch a list of public nodes so I can choose one"* and nothing more:
/// it lists, it probes, it switches. **Two column groups**, headed, so the table itself states
/// §2n.2's distinction — what monero.fail says, from monero.fail's vantage point, and what this
/// machine measured — and the two are never combined into a score fmonero would have to defend.
fn picker_pane(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Public nodes ", false, frame) else {
        return;
    };
    if inner.height == 0 {
        return;
    }
    // **The no-cache state** (revision 8). This is the screen `tui --node` and the bar widget's
    // `Choose node…` button land a first-time user on, so it gets §7a.5's treatment rather than an
    // empty table under a footer rendered against a `fetched_at` that does not exist.
    if m.picker.list.is_none() {
        let rows = Flex::vertical()
            .constraints([Constraint::Fixed(1), Constraint::Fixed(1), Constraint::Fill])
            .split(inner);
        para("No list of public nodes yet.".to_string(), th.text(), false).render(rows[0], frame);
        chips(m, rows[1], &[list_chip(m)], frame);
        lines_at(
            vec![
                Line::styled(
                    "Getting the list tells monero.fail your IP address. Nothing else is \
                     contacted.",
                    th.dimmed(),
                ),
                Line::styled(m.picker.err.clone().unwrap_or_default(), th.err()),
            ],
            rows[2],
            true,
            frame,
        );
        return;
    }
    let configured = fmonero_core::nodes::configured_address(&m.ctx);
    let rows_data = m.picker_rows();
    let probed_any = rows_data.iter().any(|n| n.probe.is_some());
    // Columns drop from the RIGHT at the narrower tiers, and **the address is never elided at any
    // tier** (§7a.4) — for the reason the send confirm never elides one.
    let (health, checked, probe_cols) = match tier {
        Tier::Wide => (true, true, true),
        Tier::Medium => (true, false, true),
        _ => (true, false, probed_any),
    };
    let mut header: Vec<&'static str> = vec!["address"];

    // **V7: no two headers read the same.** At the Wide tier this table shows monero.fail's
    // height and the height this machine measured itself, and both columns were headed `block` —
    // §7a.4 asked for two group headings (`monero.fail says` / `probed here`) to say which is
    // which, and one of those words is one §2o removes. Naming the columns instead answers the
    // same question in the space the table already has: `their block` is what the list says,
    // `block here` is what this machine saw. Both fit the 12-cell column.
    if health {
        header.push("reliability");
        header.push("their block");
    }
    if checked {
        header.push("last seen");
    }
    if probe_cols {
        // **M3: `ping` is not in the vocabulary** — §2o.2 calls this check `checked`, and every
        // other surface says `answered in {n} ms`. The column holds what the check found, which is
        // a time when it answered and a reason when it did not.
        header.push("checked");
        header.push("block here");
        header.push("synced");
    }
    let now = fmonero_core::now_unix();
    let cell_rows: Vec<Vec<String>> = rows_data
        .iter()
        .map(|n| {
            let mark = if configured.as_deref() == Some(n.address.as_str()) {
                "● "
            } else {
                "  "
            };
            // `web_compatible` is a CORS flag for browser wallets: it decides nothing here, so
            // v5.6 drops the marker and leaves the field to `node list --json` (§2o).
            // W17: `own`, as §7a.4 says — `yours` collided with `your own node`, which is the
            // one distinction the whole product is built on, for a public node the user merely
            // typed in. The footer explains it in four words.
            let tag = if n.custom { " own" } else { "" };
            let mut cells = vec![format!("{mark}{}{tag}", n.address)];
            if health {
                // The ratio **with its sample size**: 100 % of 3 and 72 % of 94 are not
                // comparable, and a bare percent invites the comparison (§2n.1). A `custom` row
                // renders `—` here because monero.fail has never heard of it.
                // **V7: `72% of 18`, which is §7a.4's own text and fits.** `"{}% of {c}
                // checks"` is 15 cells wide in a 12-cell column, so every row this table has ever
                // drawn ended `ch` — a ratio clipped into a fragment, in the column whose whole
                // purpose is that the sample size travels with the percentage (§2n.1).
                cells.push(match (n.health_ratio, n.health_checks) {
                    (Some(r), Some(c)) => format!("{}% of {c}", (r * 100.0).round() as i64),
                    _ => "—".into(),
                });
                // W7: grouped, like the pane sixty columns to the right — and `3 765 172` is
                // nine cells in a twelve-cell column, so it fits where `3765172` merely did not
                // read as a block number.
                cells.push(
                    n.health_last_height
                        .map(grouped)
                        .unwrap_or_else(|| "—".into()),
                );
            }
            if checked {
                cells.push(match (n.health_checked_at, &n.health_checked_at_text) {
                    (Some(t), _) => age((now - t).max(0) as u64),
                    (None, Some(text)) => fit(text, 16),
                    _ => "—".into(),
                });
            }
            if probe_cols {
                match &n.probe {
                    None => {
                        cells.push("—".into());
                        cells.push("—".into());
                        cells.push("—".into());
                    }
                    Some(p) if p.reachable => {
                        // With its unit, as `node list`'s table says it (one vocabulary): the cell
                        // read `20` under a header reading `ping`.
                        cells.push(
                            p.latency_ms
                                .map(|ms| format!("{ms} ms"))
                                .unwrap_or_else(|| "—".into()),
                        );
                        cells.push(p.height.map(grouped).unwrap_or_else(|| "—".into()));
                        cells.push(match p.synchronized {
                            Some(true) => "yes".into(),
                            Some(false) => "no".into(),
                            None => "—".into(),
                        });
                    }
                    // A failure is a result, and its reason is the useful half (§2n.4): the five
                    // hosts that timed out from this machine are the most useful rows in a table
                    // monero.fail rated fine.
                    Some(p) => {
                        cells.push(probe_reason(p.reason).to_string());
                        cells.push("—".into());
                        cells.push("—".into());
                    }
                }
            }
            cells
        })
        .collect();
    let body = if m.picker.filter_open {
        let split = Flex::vertical()
            .constraints([Constraint::Fixed(1), Constraint::Fill])
            .split(inner);
        search_row(
            &m.picker.filter,
            "filter by address",
            true,
            th,
            split[0],
            frame,
        );
        split[1]
    } else {
        inner
    };
    // **W6: the footer wraps, it is never cut.** It carries what was left out of the list, and
    // `fit` clipped that to `37 without a port were …` in the narrower tier — the clause exists
    // to tell the reader why a count is short, so a cut one is worse than none. Its height is the
    // number of rows its own text needs, up to three.
    let footer_text = picker_footer(m, &rows_data);
    let footer_lines: Vec<String> = footer_text
        .as_deref()
        .map(|t| wrap_words(t, body.width as usize))
        .unwrap_or_default();
    let footer_h = (footer_lines.len().clamp(1, 3)) as u16;
    // X32: the chip row takes the rows its chips need at this width, up to two.
    //
    // **M9: `[l] Use your own node` is not offered when you already are.** In local mode the chip
    // rendered and pressing it answered *"You are already using your own node."* — a control that
    // advertises a route which cannot exist in the state it is drawn in, which is the same argument
    // §6.5 makes for the widget's own Start/Stop buttons.
    let mut picker_chips: Vec<(Button, &str, &str)> = vec![
        (Button::Probe, "p", "Check these"),
        list_chip(m),
        (Button::UseNode, "Enter", "Use this node"),
        (Button::AddNode, "a", "Add a node"),
        (Button::PrevNode, "H", "Back to the last one"),
    ];
    if m.remote_mode() {
        // v5.6, §2o.1: the destination the old refusal named as a shell command.
        picker_chips.push((Button::UseLocal, "l", "Use your own node"));
    }
    let chip_h = chips_height(&picker_chips, body.width).min(2);
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fill,
            Constraint::Fixed(footer_h),
            Constraint::Fixed(chip_h),
        ])
        .split(body);
    // **The address is never elided at any tier** (§7a.4), so columns are dropped from the RIGHT
    // until it has room — rather than handed `Constraint::Fill` over whatever is left, which at 72
    // columns gave the address **zero** width and named none of the nodes it rated. Two
    // corrections (DECISIONS: DEFECT B), which is how 150×40 elided the owner's own address
    // while 100 columns rendered it whole. · `Table` spends one cell **between** columns
    // (`column_spacing` defaults to 1 and becomes the `Flex` gap —
    // `ftui-widgets-0.7.0/src/table.rs:114`, `:845`), so a kept column costs `COL + 1`, and a
    // budget counting only `COL` left the address `header.len() - 1` cells short. · And the
    // minimum is **measured**: 26 was under the 31 cells `● xmr-node.cakewallet.com:18081` needs.
    // It is the widest address cell drawn — capped at half the pane, so one 49-column hostname
    // cannot delete every data column, and never below the configured row's own width.
    const COL: u16 = 12;
    const PER_COL: u16 = COL + 1;
    const ADDRESS_FLOOR: u16 = 26; // what to budget when the list is empty: nothing to measure
    let mut widest = ADDRESS_FLOOR;
    let mut mine = 0;
    for (cells, n) in cell_rows.iter().zip(rows_data.iter()) {
        let w = cells.first().map_or(0, |s| s.width() as u16);
        widest = widest.max(w);
        if configured.as_deref() == Some(n.address.as_str()) {
            mine = w;
        }
    }
    let address_min = widest.min(rows[0].width / 2).max(mine);
    while header.len() > 1 && rows[0].width < address_min + PER_COL * (header.len() as u16 - 1) {
        header.pop();
    }
    let keep = header.len();
    let table_rows: Vec<TableRow> = cell_rows
        .into_iter()
        .map(|mut cells| {
            cells.truncate(keep);
            TableRow::new(cells)
        })
        .collect();
    let mut widths: Vec<Constraint> = vec![Constraint::Fill];
    for _ in 1..keep {
        widths.push(Constraint::Fixed(COL));
    }
    let t = Table::new(table_rows, widths)
        .header(TableRow::new(header.clone()).style(th.dimmed()))
        .style(th.text())
        .highlight_style(th.selected());
    let mut state = m.picker.table.borrow_mut();
    StatefulWidget::render(&t, rows[0], frame, &mut state);
    // One place computes a row index, and it is `hit.rs` — given the geometry recorded here
    // (§7a.2). `+1` for the header row the table draws above its first data row.
    m.hits.pane(
        HitPane::Picker,
        rows[0],
        rows[0].y + 1,
        rows[0].height.saturating_sub(1),
        state.offset,
    );
    drop(state);
    if !footer_lines.is_empty() {
        let style = if m.picker.progress.is_some() {
            th.warn()
        } else {
            th.dimmed()
        };
        lines_at(
            footer_lines
                .into_iter()
                .take(footer_h as usize)
                .map(|l| Line::styled(l, style))
                .collect(),
            rows[1],
            false,
            frame,
        );
    }
    chip_row(m, rows[2], &picker_chips, frame);
}

/// The picker's footer (§2n.1, W6) — what the list is, how old it is, and **what was left out**.
///
/// A list that silently showed 158 of 195 would look like a truncated fetch, so the two skip
/// counts surface here. v5.6 merged them into one number and dropped the noun, which left
/// `39 skipped` — a count of nothing a reader can name. Each clause names what it counts, each
/// pluralises, and a count of zero draws no clause at all.
fn picker_footer(m: &Tui, rows_data: &[&fmonero_core::PublicNode]) -> Option<String> {
    let list = m.picker.list.as_ref()?;
    if let Some(p) = &m.picker.progress {
        return Some(p.clone());
    }
    let n = rows_data.len();
    let mut parts = vec![
        format!(
            "{n} {} from monero.fail",
            if n == 1 { "node" } else { "nodes" }
        ),
        // `list {age} old` put "ago" and "old" in one phrase — `list 3s ago old` (V6). `age()`
        // already ends in "ago", so the preposition is all that was needed.
        format!(
            "list fetched {}",
            list.cache_age_sec
                .map(age)
                .unwrap_or_else(|| "an unknown time ago".into())
        ),
    ];
    if list.skipped_no_port > 0 {
        parts.push(format!(
            "{} without a port {} left out",
            list.skipped_no_port,
            if list.skipped_no_port == 1 {
                "was"
            } else {
                "were"
            }
        ));
    }
    if list.skipped_invalid > 0 {
        // **M25: a count in front of the program's name read as one phrase.** `2 fmonero could not
        // read` is what `{n} fmonero could not read` produces in a row of `·`-separated clauses.
        parts.push(format!("{} could not be read", list.skipped_invalid));
    }
    if rows_data.iter().any(|r| r.custom) {
        // W17's legend, under the table that draws the marker.
        parts.push("own = a node you added".to_string());
    }
    Some(parts.join(" · "))
}

fn node_hero(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Node ", m.status_err.is_none(), frame) else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    let mut gauge = None;
    if let Some(why) = &m.status_err {
        lines.push(Line::styled(
            format!("fmonero could not say how things are. {why}"),
            th.err(),
        ));
        lines.push(Line::styled("Press r to try again.", th.dimmed()));
    } else if let Some(s) = &m.status {
        let n = &s.node;
        lines.push(Line::from_spans([
            Span::styled("● ", th.badge(m.node_tone())),
            Span::styled(m.node_phrase(), th.title()),
        ]));
        // In remote mode the hero is a different, much smaller one (§2m.3, §7a): no gauge, no
        // percent, no ETA, no peers, no version. All of them are `null` here, and a gauge at
        // 0 % is the rendered untruth §2j forbids.
        // **With a public node the hero is two lines and no disclaimers** (§7a.3, v5.6). The
        // row that showed the trust flag and the ssl mode is **deleted**: fmonero never marks a
        // public node trusted, so there was no choice to display, and both words are ones §2o
        // removes. So is the sentence *"fmonero does not talk to this node — monero-wallet-rpc
        // does…"*, which was an architecture lecture standing where the user's answer belongs.
        // What *is* known is in the ` This node ` pane beside it.
        if n.layout == Layout::Remote {
            if s.wallet.open {
                // **V5: the same two formatters the ` This node ` pane sixty columns to the right
                // uses.** This row printed `block 3541122 · checked 2026-09-19T00:20:07+02` — a
                // block number with no grouping (§2o rule 9) beside an RFC3339 timestamp used as
                // prose and clipped mid-offset, with `checked` doing double duty for the node's own
                // measurement two lines away. One label per thing: the node is *checked*, a wallet
                // is *refreshed*.
                // W5: `refreshed not yet` is not a sentence. The clause is the whole value, so
                // it reads either `refreshed 2m 04s ago` or `not refreshed yet`.
                lines.push(kv(
                    th,
                    "this wallet",
                    format!(
                        "block {} · {}",
                        match s.wallet.height {
                            Some(h) => grouped(h),
                            None => "—".into(),
                        },
                        match refreshed_age(s) {
                            Some(age) => format!("refreshed {age}"),
                            None => "not refreshed yet".to_string(),
                        }
                    ),
                ));
                // Decision 7, on the screen a user watches while it happens: the scanning is
                // inside the wallet service, so closing this window does not stop it. **One
                // sentence, from core** (W10), and wrapped — this hero is 52% of the width.
                if let Some(line) = wallet_sync_line(s) {
                    lines.push(Line::styled(line, th.warn()));
                    lines.extend(
                        wrap_words(fmonero_core::SYNC_KEEPS_GOING, inner.width as usize)
                            .into_iter()
                            .map(|l| Line::styled(l, th.dimmed())),
                    );
                }
            }
            lines_at(lines, inner, false, frame);
            return;
        }
        if n.reachable != Some(true) {
            // Word-wrapped, not clipped: the hero is rendered `WrapMode::None` for its `kv`
            // rows, and at 150 columns this 75-character line came out as
            // `fmonero setup --unit` — a command that looks complete and exits 2.
            //
            // **W9: three states, and the advice follows the state.** While `u` runs this said
            // `Your node is not running. Press u to start it.` under a hero already reading
            // `Starting your node…` and a status bar counting the seconds — advice to press the
            // key that was running. A service that is up and silent gets the same treatment: it
            // is not started by pressing start.
            // **X23: the badge one row above already says the state**, so this row is the action
            // alone. `● Your node is not running` over `Your node is not running. Press u to start
            // it.` was the same sentence twice in two adjacent rows.
            let text = if m.monero_missing() {
                "Monero is not installed on this machine.  In a terminal: omarchy pkg add monero"
            } else {
                match m.node_off_words() {
                    "starting" => "This can take a moment. Esc stops waiting.",
                    "not answering" => "It has not answered yet. Press r to check again.",
                    _ => "Press u to start it.",
                }
            };
            lines.extend(
                wrap_words(text, inner.width as usize)
                    .into_iter()
                    .map(|l| Line::styled(l, th.warn())),
            );
        }
        if let Some(pct) = n.sync_percent {
            // W7: both numbers grouped. `100%  3541122 / 3541122` was the one place the hero
            // printed a block number the way a phone number is printed.
            let target = match (n.target_height, n.height) {
                (Some(0), Some(h)) => grouped(h),
                (Some(t), _) => grouped(t),
                _ => "—".into(),
            };
            gauge = Some((lines.len() as u16, pct / 100.0));
            let here = n.height.map(grouped).unwrap_or_else(|| "—".into());
            lines.push(kv(
                th,
                "blocks",
                // X11: one rounding for the whole program, from core.
                format!("{:13}{}  {here} / {target}", "", fmonero_core::percent(pct)),
            ));
            if let Some(eta) = m.sync_eta() {
                lines.push(kv(th, "done in", eta));
            }
        }
        // **X23: three rows of dashes under a hero that has already said why.** With your own node
        // off this drew `last block —` / `peers — out · — in` / `version — · —` beneath
        // `Your node is not running. Press u to start it.` — the shape §6.5 deleted from the widget
        // and W3 deleted from `status`.
        //
        // While the node is RUNNING all three are drawn, dashes included: a restricted node is
        // running and answers none of them, and the hero's own note two lines below is what
        // explains those dashes (§5's rule that unknown is `—` and never a zero).
        if n.reachable == Some(true) {
            lines.push(kv(
                th,
                "last block",
                n.top_block_age_sec
                    .map_or("—".into(), |a| age(a.max(0) as u64)),
            ));
            lines.push(kv(
                th,
                "peers",
                format!(
                    "{} out · {} in",
                    opt(n.outgoing_connections_count),
                    opt(n.incoming_connections_count)
                ),
            ));
            // The hard-fork version is an internal number no user acts on; it stays in
            // `node info --json` (§2o).
            lines.push(kv(
                th,
                "version",
                format!(
                    "{} · {}",
                    n.version.clone().unwrap_or_else(|| "—".into()),
                    n.nettype.clone().unwrap_or_else(|| "—".into())
                ),
            ));
        }
        if n.layout == Layout::SystemUnit {
            // One sentence, shared with core, so the refusal and the hero cannot drift (§2o).
            lines.extend(
                wrap_words(fmonero_core::FOREIGN_NODE_READ_ONLY, inner.width as usize)
                    .into_iter()
                    .map(|l| Line::styled(l, th.warn())),
            );
        }
        if n.restricted == Some(true) {
            lines.extend(
                wrap_words(
                    "This node only answers basic questions, so peers and traffic are not shown.",
                    inner.width as usize,
                )
                .into_iter()
                .map(|l| Line::styled(l, th.warn())),
            );
        }
    } else {
        lines.push(Line::styled("checking…", th.dimmed()));
    }
    // The `u`/`d` chips, **in local mode only** (§7a.2's rule that a key hint is a button, and
    // §6.5's rule that a control which can never be enabled here teaches the user the app is
    // broken): in remote mode both commands exit 4 `remote_node`, and `Choose node…` — the picker
    // below — is what stands in their place.
    let local = m.status.as_ref().map(|s| s.node.layout) != Some(Layout::Remote);
    let used = lines.len() as u16;
    if local && inner.height > used {
        chip_row(
            m,
            Rect::new(inner.x, inner.y + used, inner.width, 1),
            &[
                (Button::NodeUp, "u", "Start your node"),
                (Button::NodeDown, "d", "Stop your node"),
            ],
            frame,
        );
    }
    lines_at(lines, inner, false, frame);
    if let Some((row, value)) = gauge {
        if row < inner.height && inner.width > 30 {
            MiniBar::new(value.clamp(0.0, 1.0), 12)
                .colors(MiniBarColors::new(th.red, th.yellow, th.green, th.green))
                .render(Rect::new(inner.x + 11, inner.y + row, 12, 1), frame);
        }
    }
}

/// A block number as a person reads it (§2o rule 9). **One implementation for the whole program**
/// — [`fmonero_core::grouped`], which W7 moved into core so that core's own warnings, the hero,
/// the status bar, the picker, the doctor's details and the widget's `Model.grouped` all render
/// one number one way (§2a.3: an invariant that lives in two places holds in one).
pub(super) use fmonero_core::grouped;

/// How long ago fmonero last checked this wallet for payments, in the node pane's own words
/// (v5.6, V5). `None` when no refresh has been recorded, which the caller renders as
/// `not refreshed yet` rather than as `—`: the dash means "unknown", and this is known and simply
/// has not happened (W5).
fn refreshed_age(s: &Status) -> Option<String> {
    let at = fmonero_core::unix_from_rfc3339(s.wallet.last_refresh_at.as_deref()?)?;
    Some(age((fmonero_core::now_unix() - at).max(0) as u64))
}

/// Decision 7's line, on both screens that render it and in the widget, from one formula (W22
/// moved this comment back onto the function it describes: `refreshed_age` was inserted above it).
///
/// `None` when the wallet is not behind, or when either height is unknown: a percentage computed
/// from a missing node height would be the rendered untruth §2j forbids. The node's height is the
/// live `get_info` with your own node and the **cached check's** with a public one, which is the
/// only source there is (§5's `node.remote_probe`).
pub(super) fn wallet_sync_line(s: &Status) -> Option<String> {
    let wallet = s.wallet.height?;
    let node = s.node.height.or_else(|| {
        s.node
            .remote_probe
            .as_ref()
            .filter(|p| p.reachable)
            .and_then(|p| p.height)
    })?;
    if wallet == 0 || node <= wallet {
        return None;
    }
    let pct = wallet.saturating_mul(100) / node.max(1);
    Some(format!(
        "Syncing {pct}% · {} blocks left.",
        grouped(node - wallet)
    ))
}

/// §7a.3's ` This node ` pane (v5.6) — **the whole of decision 3**.
///
/// With a public node v5.5 rendered two panes whose entire content was a sentence about what
/// they could not show. This one renders what *is* known: the four facts of the cached check
/// (§2n.4), which is a measurement fmonero already had and never showed here, and the way out.
/// The height and its age always render **together**: a four-hour-old `synced` says nothing
/// about now.
fn this_node_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " This node ", false, frame) else {
        return;
    };
    let probe = m.status.as_ref().and_then(|s| s.node.remote_probe.as_ref());
    let mut lines: Vec<Line> = Vec::new();
    let chip: (Button, &str, &str) = match probe {
        None => {
            lines.push(Line::styled("Not checked yet.", th.dimmed()));
            (Button::Probe, "p", "Check this node")
        }
        Some(p) if !p.reachable => {
            // M3: the same word the hero, the badge and the picker's column use.
            lines.push(Line::styled(
                format!(
                    "{} when checked {}.",
                    sentence_case(fmonero_core::nodes::probe_reason_words(p.reason.as_deref())),
                    age(p.age_sec.max(0) as u64)
                ),
                th.warn(),
            ));
            list_chip(m)
        }
        Some(p) => {
            lines.push(Line::styled(
                match p.height {
                    Some(h) => format!("block {}", grouped(h)),
                    None => "it answered, but did not say which block".to_string(),
                },
                th.title(),
            ));
            lines.push(Line::styled(
                match p.synchronized {
                    Some(true) => "synced".to_string(),
                    Some(false) => "syncing".to_string(),
                    None => "it did not say whether it is synced".to_string(),
                },
                th.text(),
            ));
            if let Some(ms) = p.latency_ms {
                lines.push(Line::styled(format!("answered in {ms} ms"), th.dimmed()));
            }
            lines.push(Line::styled(
                format!("checked {}", age(p.age_sec.max(0) as u64)),
                th.dimmed(),
            ));
            list_chip(m)
        }
    };
    let used = lines.len() as u16;
    lines_at(lines, inner, false, frame);
    if inner.height > used {
        chip_row(
            m,
            Rect::new(inner.x, inner.y + used, inner.width, 1),
            &[chip],
            frame,
        );
    }
}

/// The Node screen's list pane: connections, or the bans `b` swaps to.
fn list_pane(m: &Tui, area: Rect, frame: &mut Frame, wide: bool) {
    let th = &m.theme;
    let bans = m.node.view == NodeView::Bans;
    let extras = m.node.extras.as_ref();
    // **M16: a key belongs in a chip, not inside a pane title.** These read ` Peers  0 · b: bans `
    // and ` Bans  0 · b: peers ` — a style used nowhere else in the product, and the one place a
    // reader meets `b:` as a prefix. The title names what is in the pane and the line under the
    // table says what `b` does, in words.
    let title = if bans {
        format!(
            " Blocked addresses  {} ",
            extras.map_or(0, |e| e.bans.len())
        )
    } else {
        format!(" Peers  {} ", extras.map_or(0, |e| e.connections.len()))
    };
    // **M16: the swap is a chip under the pane, like every other key in the product.** It was a
    // `b:` prefix inside the pane's own title — a style used nowhere else, and the one place in
    // the product where a key appeared without a chip around it, so it was also not clickable.
    let swap = if bans {
        (Button::SwapPeers, "b", "Show the peers")
    } else {
        (Button::SwapPeers, "b", "Show blocked addresses")
    };
    // **Inside the pane, on its last interior row** — the ` This node ` pane puts `[p] Check this
    // node` there and that is the pattern. The first version of this put it in the gap BELOW the
    // pane, flush left and outside every frame, which is the one shape V3 deleted from the Wallet
    // screen for being the only unframed text in the product.
    let empty = |text: &str, frame: &mut Frame| {
        if let Some(inner) = framed(area, th, &title, false, frame) {
            let rows = Flex::vertical()
                .constraints([Constraint::Fill, Constraint::Fixed(1)])
                .split(inner);
            para(text.to_string(), th.dimmed(), true).render(rows[0], frame);
            chip_row(m, rows[1], &[swap], frame);
        }
    };
    if m.remote_mode() {
        empty("This is about your own node.", frame);
        return;
    }
    if m.node_restricted() {
        // The method name was an internal identifier in a sentence (§2o rule 5).
        empty("This node does not answer that.", frame);
        return;
    }
    let Some(extras) = extras else {
        // W9: `checking the node…` is true while a first answer is on its way and false once the
        // document says the node is off — and the pane sat on it beside a hero that had already
        // said so. The sentence comes from the one predicate; the error, when there is one, wins.
        let off = m
            .status
            .as_ref()
            .is_some_and(|s| s.node.reachable != Some(true));
        let text = if off {
            // The one predicate wins over the transport's own words: `The node is not answering.`
            // is what a refused connection says, and it stood in three panes beside a hero
            // reading `Your node is not running` (W9).
            // **X23: and the pane says what is missing from IT**, so the frame states the node's
            // own state once, in the hero.
            if bans {
                "No bans to show while your node is off.".to_string()
            } else {
                "No peers while your node is off.".to_string()
            }
        } else {
            m.node
                .err
                .clone()
                .unwrap_or_else(|| "checking the node…".into())
        };
        empty(&text, frame);
        return;
    };
    if bans {
        if extras.bans.is_empty() {
            // §2o.1: there is no in-app ban key, so the pane states the fact and stops rather
            // than printing a command at a user who cannot type it (docs/COPY.md §2).
            empty("No banned peers.", frame);
            return;
        }
        let rows = extras
            .bans
            .iter()
            .map(|b| TableRow::new([fit(&b.host, 24), age(b.seconds).replace(" ago", " left")]));
        table(
            m,
            area,
            &title,
            rows.collect(),
            &["HOST", "REMAINING"],
            &[25],
            frame,
        );
        swap_chip(m, area, swap, frame);
        return;
    }
    if extras.connections.is_empty() {
        empty("No peers yet.", frame);
        return;
    }
    let rows = extras
        .connections
        .iter()
        .take(10)
        .map(|c| {
            // W7 reaches here too: this table printed `3541122` in the same frame as the hero's
            // `3 541 122`. · The pruning seed is **gone** from the row: `KEEPS 0` and `KEEPS 384`
            // decide nothing a reader can act on, which is the argument `docs/COPY.md` §3.9 already
            // made for `node peers`' own human output. It stays in `node peers --json`.
            // **X29: the node's own `synchronizing` reached this column** against `synced`
            // everywhere else in the program (§2o.2 fixes `catching up`), and so did
            // `before_handshake`. One table, shared with `node peers`' own output.
            let mut cells = vec![
                fit(&c.host, 22),
                crate::output::peer_state_words(Some(&c.state)),
                grouped(c.height),
            ];
            if wide {
                // X29: `2.1 kB/0 B` under a header reading `AVG ↓/↑` is a rate with no time unit.
                cells.push(format!("{} / {}", rate(c.avg_download), rate(c.avg_upload)));
            }
            TableRow::new(cells)
        })
        .collect();
    let (header, widths): (&[&str], &[u16]) = if wide {
        (
            &["HOST", "DOING", "BLOCK", "AVERAGE DOWN / UP"],
            &[23, 14, 11],
        )
    } else {
        (&["HOST", "DOING", "BLOCK"], &[23, 14])
    };
    table(m, area, &title, rows, header, widths, frame);
    swap_chip(m, area, swap, frame);
}

/// The `b` chip on the peers/blocked-addresses pane's **last interior row** (M16).
///
/// `table` draws the whole pane, so the chip is written over the row inside the bottom border —
/// which is the row the table's own `Fill` leaves empty whenever the list is shorter than the pane,
/// and the list is at most ten rows (`take(10)`) in a pane that is taller than that at both sizes.
fn swap_chip(m: &Tui, area: Rect, swap: (Button, &str, &str), frame: &mut Frame) {
    if area.height < 4 || area.width < 6 {
        return;
    }
    let row = Rect::new(
        area.x + 1,
        area.bottom().saturating_sub(2),
        area.width.saturating_sub(2),
        1,
    );
    chip_row(m, row, &[swap], frame);
}

/// **Never rendered with a public node** (§7a.3, v5.6): v5.5's version of this pane was a
/// frame around the sentence *"not available with a remote node"*, and `node_screen` now lays
/// out ` This node ` there instead.
fn net_pane(m: &Tui, area: Rect, frame: &mut Frame, sparkline: bool) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Net ", false, frame) else {
        return;
    };
    // W9, and §6.5's rule that a row carrying no information for this state does not belong on
    // the screen: with the node off this pane drew `↓ ↑  —  —` over `speed limits ↑— ↓— kB/s`,
    // four dashes and a unit, beside a hero that had already said why.
    let off = m
        .status
        .as_ref()
        .is_some_and(|s| s.node.reachable != Some(true));
    let net = m.status.as_ref().map(|s| &s.node.net).filter(|_| !off);
    let Some(n) = net else {
        // **X23: one statement per frame.** This repeated the hero's own sentence, as did the
        // waiting pane and the peers pane, so `Your node is not running.` stood in a frame six
        // times. A pane says what is missing from IT; the hero says why.
        para(
            if off {
                "No traffic while your node is off.".to_string()
            } else {
                m.node
                    .err
                    .clone()
                    .unwrap_or_else(|| "checking the traffic…".into())
            },
            th.dimmed(),
            false,
        )
        .render(inner, frame);
        return;
    };
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(2), Constraint::Fill])
        .split(inner);
    // **X29: `↓ ↑  0 B/s  0 B/s` and `speed limits ↑2048 ↓8192 kB/s` had no words at all**, and the
    // second put its unit once at the end of two numbers going in opposite directions. Both rows
    // say which way the traffic is going, and every number carries its own unit.
    let bps = |v: Option<u64>| v.map_or("—".to_string(), rate);
    let limit = |v: Option<i64>| match v {
        Some(n) if n < 0 => "no limit".to_string(),
        Some(n) => format!("{n} kB/s"),
        None => "—".to_string(),
    };
    lines_at(
        vec![
            kv(
                th,
                "right now",
                format!("{} down · {} up", bps(n.rx_bps), bps(n.tx_bps)),
            ),
            // **M16: `limits` said nothing about whose or who set them.** They are the node's own
            // speed limits, from its configuration, and the label says so.
            kv(
                th,
                "its limits",
                format!(
                    "{} down · {} up",
                    limit(n.limit_down_kbps),
                    limit(n.limit_up_kbps)
                ),
            ),
        ],
        rows[0],
        false,
        frame,
    );
    if sparkline && rows[1].height > 0 {
        let data: Vec<f64> = m.node.rx.iter().copied().collect();
        if data.len() > 1 {
            Sparkline::new(&data).style(th.ok()).render(rows[1], frame);
        } else {
            para("The graph fills in as it measures.", th.dimmed(), true).render(rows[1], frame);
        }
    }
}

/// **Never rendered with a public node**, for [`net_pane`]'s reason.
fn pool_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    // **S3: the title says what the pane is and nothing about units.**
    //
    // It read ` Waiting to be mined · fees in XMR per byte `, and the row immediately under it was
    // `their fees  0.000427 XMR` — the TOTAL fees of everything waiting, about four orders of
    // magnitude away from a per-byte rate, sitting one row above four genuine per-byte rates under
    // a title claiming the unit for all of them. The unit belongs on the rows it is true of, and
    // the row that is a total says so in words.
    let Some(inner) = framed(area, th, " Waiting to be mined ", false, frame) else {
        return;
    };
    let mut lines = Vec::new();
    match m.node.extras.as_ref().and_then(|e| e.txpool.as_ref()) {
        Some(p) => {
            let oldest = if p.oldest == 0 {
                "—".to_string()
            } else {
                age(fmonero_core::now_unix()
                    .saturating_sub(p.oldest as i64)
                    .max(0) as u64)
            };
            // `payments`, and the count without the word `transactions`: this row is 45 cells
            // wide in a 42-cell pane at the Wide tier, so it wrapped onto a line reading `ago`
            // — and the row under it then fell off the pane's bottom (self-read, round 3).
            lines.push(kv(
                th,
                "waiting",
                format!(
                    "{} {} · {} · oldest {oldest}",
                    p.txs_total,
                    if p.txs_total == 1 {
                        "payment"
                    } else {
                        "payments"
                    },
                    bytes(p.bytes_total)
                ),
            ));
            // **S3: this is a total and the row says so**, in the CLI's own words for the same
            // figure (`they take 21.4 kB and carry 0.000427 XMR in fees`). `their fees` under a
            // title that said `per byte` was the whole of the confusion.
            //
            // **M16: `0 being rejected` is gone from the row.** "Being rejected" is a phrase a
            // reader has to look up, it is zero on every healthy node, and the clause said nothing
            // about what to do. It is stated only when it is not zero, and then as a sentence.
            // `num_failing` stays in `node info --json` for an agent either way.
            //
            // **X29: `0 double spends` is gone** for the same reasons.
            lines.push(kv(
                th,
                "they carry",
                format!("{} XMR in fees altogether", p.fee_total.to_xmr_string()),
            ));
            if p.num_failing > 0 {
                lines.push(kv(
                    th,
                    "",
                    format!(
                        "{} of them {} being turned away",
                        p.num_failing,
                        if p.num_failing == 1 { "is" } else { "are" }
                    ),
                ));
            }
        }
        None => lines.push(Line::styled(
            // W9, as in the peers pane; X23: what is missing from THIS pane, not the node's state
            // for the fourth time in one frame.
            if m.status
                .as_ref()
                .is_some_and(|s| s.node.reachable != Some(true))
            {
                "Nothing waiting while your node is off.".to_string()
            } else {
                m.node
                    .err
                    .clone()
                    .unwrap_or_else(|| "checking what is waiting…".into())
            },
            th.dimmed(),
        )),
    }
    // Only when there is a fee to show: `fee levels  —  XMR per byte` is a label, a dash and a
    // unit (W9's sibling, §6.5). **The label carries the unit** (S3): these four ARE per-byte
    // rates, the pane title no longer claims it for the rows above them, and four repetitions of
    // `XMR` on one 42-cell row do not fit.
    if let Some(f) = m
        .status
        .as_ref()
        .map(|s| &s.node.fee)
        .filter(|f| f.fees_pico.is_some())
    {
        // The four levels by the **names the Send screen offers**, so the two screens agree
        // (§2o): the old row labelled them by their priority index, which is a number nobody
        // chooses between.
        let fees: Vec<String> = f.fees_pico.as_ref().map_or_else(
            || vec!["—".to_string()],
            |v| {
                v.iter()
                    .enumerate()
                    // **M4: eight decimals, so the four line up.** `to_xmr_string` trims to four,
                    // which rendered the largest of the four in the fewest digits —
                    // `fast 0.00000032 · fastest 0.000004` — on the row a person reads to choose
                    // between them.
                    .map(|(i, p)| {
                        format!(
                            "{} {}",
                            FEE_LEVELS[i.min(3)],
                            p.to_xmr_min(fmonero_core::Piconero::FEE_DECIMALS)
                        )
                    })
                    .collect()
            },
        );
        // **Two rows when one does not hold them** (self-read, round 3): all four levels are 74
        // cells and this pane is 42 at the Wide tier, so the third and fourth were cut with nothing
        // to say they existed — in the pane a user reads to choose between them. At 150 columns one
        // row holds all four, and then splitting would be an arbitrary break.
        let room = (inner.width as usize).saturating_sub(12);
        let all = fees.join(" · ");
        if all.width() <= room {
            lines.push(kv(th, "XMR a byte", all));
        } else {
            let mut pairs = fees.into_iter();
            let first: Vec<String> = pairs.by_ref().take(2).collect();
            let rest: Vec<String> = pairs.collect();
            lines.push(kv(th, "XMR a byte", first.join(" · ")));
            if !rest.is_empty() {
                lines.push(kv(th, "", rest.join(" · ")));
            }
        }
    }
    lines_at(lines, inner, true, frame);
}

fn warnings_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    // ` Notes `, not ` Warnings `: two of the three severities are `info`, and the pane was
    // shouting at a correct machine (§2o).
    let Some(inner) = framed(area, th, " Notes ", false, frame) else {
        return;
    };
    let empty = Vec::new();
    let list = m.status.as_ref().map_or(&empty, |s| &s.warnings);
    if list.is_empty() {
        para("Nothing to report.", th.dimmed(), false).render(inner, frame);
        return;
    }
    // Most severe first: this pane can be three rows tall, and the entry that gets cut must
    // never be the `error`.
    let mut ordered: Vec<_> = list.iter().collect();
    ordered.sort_by_key(|w| match w.severity {
        Severity::Error => 0u8,
        Severity::Warn => 1,
        Severity::Info => 2,
    });
    let mut lines = Vec::new();
    for w in ordered {
        let style = match w.severity {
            Severity::Error => th.err(),
            Severity::Warn => th.warn(),
            Severity::Info => th.text(),
        };
        // **The id is gone and the action replaces the hint** (§2o rule 5, §2o.1): every
        // warning used to be printed with its internal identifier in front of it and a shell
        // command under it, in a panel nobody can type into.
        //
        // **X15: a bullet that wraps keeps its hanging indent.** `WrapMode::Word` returns every
        // continuation line to column 0, so at 120 columns `Press o on the Wallet screen to open a`
        // was indented two and `wallet.` was not — a wrapped bullet read as a new bullet, and every
        // bullet long enough to wrap did it. The wrapping happens here instead, at the pane's own
        // width, and the paragraph is rendered unwrapped.
        let text_w = (inner.width as usize).saturating_sub(2).max(8);
        for (i, part) in wrap_words(&w.text, text_w).into_iter().enumerate() {
            if i == 0 {
                lines.push(Line::from_spans([
                    Span::styled("• ", style),
                    Span::styled(part, th.text()),
                ]));
            } else {
                lines.push(Line::styled(format!("  {part}"), th.text()));
            }
        }
        if let Some(action) = w.action.as_deref() {
            for (i, part) in wrap_words(action, text_w).into_iter().enumerate() {
                let _ = i;
                lines.push(Line::styled(format!("  {part}"), th.dimmed()));
            }
        }
    }
    lines_at(lines, inner, false, frame);
}

// ---------------------------------------------------------------- Wallet

fn wallet_screen(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    if area.height < 4 {
        return;
    }
    // **V12: with no wallet open there is no balance to frame.** The header rendered
    // `— XMR   unlocked — XMR` above the empty state — two dashes where the one thing the screen
    // is about would go — so the empty state now owns the whole screen, and it already opens with
    // `No wallet is open.`
    let open = m.status.as_ref().is_some_and(|s| s.wallet.open);
    if !open {
        wallet_empty(m, area, frame);
        return;
    }
    // **Decision 7, said out loud** (v5.6): while a wallet is behind, the scanning is happening
    // inside the wallet service and closing this window does not stop it — exactly as
    // fbittorrent's transfers keep running when its window closes. The program has always
    // behaved this way and never said so.
    //
    // **V3: the two lines live INSIDE the ` Balance ` pane**, which is what the Node screen does
    // with the same pair. They were drawn on a bare rect in the gap between two framed panes, at
    // column 0 — the only unframed, flush-left text in the program, and it was decision 7's own
    // new text.
    let sync = m
        .status
        .as_ref()
        .and_then(wallet_sync_line)
        .filter(|_| area.height >= 8);
    let header_h = if sync.is_some() { 5 } else { 3 };
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(header_h), Constraint::Fill])
        .split(area);
    wallet_header(m, rows[0], sync.as_deref(), frame);
    let rest = rows[1];
    // Every key hint on an open wallet is a chip too (§7a.2): a hint that is not clickable is half
    // of what the owner asked for.
    let body = Flex::vertical()
        .constraints([Constraint::Fill, Constraint::Fixed(1)])
        .split(rest);
    chip_row(
        m,
        body[1],
        &[
            (Button::RefreshWallet, "R", "Check for payments"),
            (Button::Qr, "Q", "QR code"),
            (Button::CloseWallet, "x", "Close wallet"),
        ],
        frame,
    );
    match tier {
        Tier::Wide => {
            let cols = Flex::horizontal()
                .constraints([Constraint::Percentage(42.0), Constraint::Fill])
                .split(body[0]);
            subaddress_pane(m, cols[0], frame);
            history_pane(m, cols[1], frame, true);
        }
        Tier::Medium => {
            let split = Flex::vertical()
                .constraints([Constraint::Fixed(8), Constraint::Fill])
                .split(body[0]);
            subaddress_pane(m, split[0], frame);
            history_pane(m, split[1], frame, false);
        }
        _ => match m.wallet.pane {
            Pane::Subaddresses => subaddress_pane(m, body[0], frame),
            Pane::History => history_pane(m, body[0], frame, false),
        },
    }
}

/// §7a.5's empty state: **a panel, not an empty table.**
///
/// Read against v5.4, the owner's confusion was entirely earned. On a fresh remote-mode machine
/// this screen said *"Wallet daemon is not running (press o to start it and open a wallet)"* —
/// true and useless, because `o` lists the wallets in `wallet.dir` and there were none; `c` was
/// **close**, not create, so the one key that looks like "create" did nothing visible; and there
/// was no create flow in the TUI at all. Three dead ends. This is three buttons and two status
/// lines instead, and the wallet count is **real** (`wallet_dir::list`), so `Open wallet…` reads
/// `0 wallets` and the user can see *why* `o` will not help before pressing it.
fn wallet_empty(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Wallet ", true, frame) else {
        return;
    };
    if inner.height == 0 {
        return;
    }
    // **V12.** `Open wallet…      choose one of the 1 in /home/…/wallets/mainnet` said the count
    // and the path and neither in English, and *one of the 1 in* is worse than the `pick one of the
    // 1 wallets in …` it replaced. The count is what decides whether `o` can help, the names are
    // what the user is looking for, and the directory is one `fmonero settings get wallet.dir`
    // away for the one reader who wants it.
    let open_label = match m.wallets.len() {
        // **S1 first**: with the folder unreadable the count is not zero, it is unknown, and the
        // chip must not name the key that would write a new wallet file on top of an old one.
        _ if m.wallets_err.is_some() => {
            "Open a wallet      (fmonero cannot read your wallet folder. Press o to see why)"
                .to_string()
        }
        0 => "Open a wallet      (none on this machine yet. Press c to create one)".to_string(),
        n => {
            let names = m.wallets.join(", ");
            let plural = if n == 1 { "wallet" } else { "wallets" };
            let listed = format!("Open a wallet      ({n} {plural} on this machine: {names})");
            if listed.width() <= inner.width as usize {
                listed
            } else {
                format!("Open a wallet      ({n} {plural} on this machine)")
            }
        }
    };
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fixed(3),
            Constraint::Fixed(1),
            Constraint::Fill,
        ])
        .split(inner);
    para("No wallet is open.".to_string(), th.text(), false).render(rows[0], frame);
    // Every bracketed chip is a hit target (§7a.2), and the stacked form is kept at **every**
    // tier: a screen whose whole content is three buttons has nothing to drop.
    chips(
        m,
        rows[2],
        &[
            (Button::OpenWallet, "o", open_label.as_str()),
            // **X36: one style for the three chips on this screen.** This one carried an ellipsis
            // and no article while the one above it had an article and no ellipsis — W19 put the
            // create modal's own four labels in one style and left these in two.
            (
                Button::CreateWallet,
                "c",
                "Create a wallet    (a new one, kept on this machine)",
            ),
            // **W15: the label follows the state.** In the state §7a.5 says `W` stops the
            // service — it is running and nothing is open — this chip still read `Start`, and
            // pressing it stopped the service. One predicate decides both (`Tui::
            // wallet_daemon_key_stops`).
            // S9: one predicate decides the label, and it knows about an action in flight.
            (Button::WalletDaemon, "W", m.wallet_daemon_chip()),
        ],
        frame,
    );
    // The daemon line is one of exactly four strings, and the node line is there because "which
    // node" and "is the daemon up" were the two questions the old screen answered in two
    // different places, one of them a refusal.
    lines_at(
        vec![
            Line::styled(
                format!("wallet service: {}", wallet_daemon_phrase(m)),
                th.dimmed(),
            ),
            Line::styled(format!("node: {}", node_phrase(m)), th.dimmed()),
        ],
        rows[4],
        true,
        frame,
    );
    let _ = (rows[1], rows[3]);
}

/// §7a.5's strings for the wallet service — **five from v5.6.1, and the fifth is V13**.
///
/// `wallet service: running` sat four rows above a status bar reading `wallet not running`, and a
/// person cannot reconcile those. Both were right: one is `unit_active`, the other `reachable`. The
/// badge now says `not answering` for that state (see [`Tui::wallet_badge`]) and this row says the
/// same thing in its own register, so the two rows are one fact in two lengths.
fn wallet_daemon_phrase(m: &Tui) -> String {
    // **S9: one description of what is happening, and it is the action's own.** Eight seconds into
    // a create this row said `running, but not answering yet` while the badge said `wallet
    // scanning` and the status bar said `Creating mywallet 8s / 300s` — three wordings of one
    // thing, in one frame. Whatever fmonero is doing right now, the label it is doing it under is
    // what every row says, so the three cannot disagree.
    if let Some(a) = m.action.as_ref().filter(|a| a.wallet_busy) {
        // The row's own subject is already `wallet service`, so an action whose label names it
        // again read `wallet service: Starting the wallet service…` (round 4's own read).
        if a.label.contains("wallet service") {
            return "starting…".into();
        }
        return format!("{}…", a.label);
    }
    let Some(s) = m.status.as_ref() else {
        return "checking…".into();
    };
    if s.wallet.unit_active {
        return if s.wallet.reachable {
            "running".into()
        } else {
            "running, but not answering yet".into()
        };
    }
    // Not `idle`, which is the program's word: the service is DESIGNED to be down until a wallet
    // is opened (§4.2 writes both units disabled), which is the same fact §5's `wallet_idle`
    // takes out of the alarm channel. The sentence is the whole point of the row, so it gets to
    // be a sentence (§2o).
    if s.node.layout == Layout::Remote && s.wallet.name.is_none() {
        "not running yet. It starts when you open or create a wallet.".into()
    } else {
        "not running".into()
    }
}

fn node_phrase(m: &Tui) -> String {
    let Some(s) = m.status.as_ref() else {
        return "—".into();
    };
    match s.node.layout {
        // The trust flag is deleted everywhere (§2o.2): fmonero never marks a public node
        // trusted, so the row showed a value the user did not choose and cannot change.
        Layout::Remote => format!("public node {}", s.node.url),
        _ => format!(
            "your own node {} · {}",
            s.node.url,
            match s.node.reachable {
                Some(true) => "running",
                Some(false) => "not running",
                None => "—",
            }
        ),
    }
}

fn wallet_header(m: &Tui, area: Rect, sync: Option<&str>, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Balance ", false, frame) else {
        return;
    };
    let Some(w) = m.status.as_ref().map(|s| &s.wallet) else {
        return;
    };
    // Decision 7's two lines, inside the frame (V3). The balance keeps the first row; these take
    // the two under it, which is why the pane is five rows tall while a wallet is catching up.
    if let Some(line) = sync {
        if inner.height >= 3 {
            // W10: the sentence is core's, so this pane, the Node hero and the bar widget cannot
            // word decision 7 three ways. Wrapped, because it is 69 characters and this pane is
            // as narrow as the terminal.
            let mut rows = vec![Line::styled(line.to_string(), th.warn())];
            rows.extend(
                wrap_words(fmonero_core::SYNC_KEEPS_GOING, inner.width as usize)
                    .into_iter()
                    .map(|l| Line::styled(l, th.dimmed())),
            );
            let h = (rows.len() as u16).min(inner.height.saturating_sub(1));
            lines_at(
                rows,
                Rect::new(inner.x, inner.y + 1, inner.width, h),
                false,
                frame,
            );
        }
    }
    let dash = || "—".to_string();
    let mut spans = vec![
        Span::styled(
            format!("{} XMR", w.balance_xmr.clone().unwrap_or_else(dash)),
            th.accented(),
        ),
        Span::styled(
            format!(
                "   unlocked {} XMR",
                w.unlocked_xmr.clone().unwrap_or_else(dash)
            ),
            th.text(),
        ),
    ];
    if let Some(blocks) = w.blocks_to_unlock.filter(|b| *b > 0) {
        // The amount, and minutes rather than blocks: blocks are the mechanism, minutes are the
        // answer (§2o rule 9).
        let locked = match (w.balance_pico, w.unlocked_pico) {
            (Some(b), Some(u)) => {
                fmonero_core::amount::Piconero(b.0.saturating_sub(u.0)).to_xmr_string()
            }
            _ => "some".to_string(),
        };
        let mins = w
            .time_to_unlock_sec
            .map_or(blocks * 2, |sec| sec.div_ceil(60));
        spans.push(Span::styled(
            format!("   {locked} XMR locked for about {mins} min"),
            th.warn(),
        ));
    }
    if let Some(name) = &w.name {
        spans.push(Span::styled(format!("   {name}"), th.dimmed()));
    }
    if let Some(behind) = w.blocks_behind.filter(|b| *b > 0) {
        spans.push(Span::styled(
            format!("   {} blocks to go", grouped(behind.max(0) as u64)),
            th.dimmed(),
        ));
    }
    // The balance keeps row 0 of the pane, whatever else the pane holds (V3).
    line_at(spans, Rect::new(inner.x, inner.y, inner.width, 1), frame);
}

fn subaddress_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    // **M6: the NAME column at 120 columns too.** The threshold was 62, and this pane is 50 cells
    // at the owner's width — so `l` renamed an address and the name it wrote was nowhere on the
    // screen. The column is eight cells at 48 and eleven from 62, and the balance beside it keeps
    // its explicit ellipsis for the one value that no longer fits whole.
    let name_w: usize = if area.width >= 62 { 11 } else { 8 };
    let wide = area.width >= 48;
    let th = &m.theme;
    let focused = m.wallet.pane == Pane::Subaddresses;
    // `Addresses`, not `Your addresses`: this pane is 48 columns and the longer title clipped to
    // `… · a new · l renam`, which is a title that lies about its own keys.
    let title = " Addresses · Enter copies · a new · l rename ";
    let Some(set) = &m.wallet.addresses else {
        if let Some(inner) = framed(area, th, title, focused, frame) {
            let (text, style) = match &m.wallet.err {
                Some(why) => (format!("{why}\nPress r to try again."), th.err()),
                None => ("checking…".to_string(), th.dimmed()),
            };
            para(text, style, true).render(inner, frame);
        }
        return;
    };
    // BALANCE is the table's `Constraint::Fill` column, so a value wider than what is left is
    // **clipped with no ellipsis** — and a clipped XMR amount is itself a well-formed XMR
    // amount, so `1.234567891234` read as `1.234567891` with nothing to notice. The width is
    // computed here (block borders, the fixed columns and the 1-column gaps) and the value is
    // trimmed with an explicit `…`, so the cell either holds the whole number or says it does
    // not (§5, §2j).
    let fixed_sum: u16 = if wide { 3 + name_w as u16 + 22 } else { 3 + 22 };
    let gaps: u16 = if wide { 3 } else { 2 };
    let bal_w = area.width.saturating_sub(2 + fixed_sum + gaps).max(1) as usize;
    let rows = set
        .addresses
        .iter()
        .map(|a| {
            // `get_balance.per_subaddress` holds only the FUNDED subaddresses (§5), so a row
            // with no entry shows `—` rather than a zero that would read as "empty".
            let bal = m
                .wallet
                .balance
                .as_ref()
                .and_then(|b| {
                    b.per_subaddress
                        .iter()
                        .find(|s| s.address_index == a.address_index)
                })
                .map_or("—".to_string(), |s| {
                    fit(&s.balance.to_xmr_string(), bal_w)
                });
            // `used` is a marker on the address rather than a column of its own: at 48
            // columns a fifth column is a cut column, and a cut `used` reads as `us`.
            let addr = format!(
                "{}{}",
                short_addr(&a.address),
                if a.used { " ✓" } else { "" }
            );
            let mut cells = vec![a.address_index.to_string()];
            if wide {
                cells.push(fit(&a.label, name_w.saturating_sub(1)));
            }
            cells.push(addr);
            cells.push(bal);
            TableRow::new(cells)
        })
        .collect();
    // X20: `BALANCE` is money and carries its unit.
    let (header, fixed): (&[&str], &[u16]) = if wide {
        (
            &["#", "NAME", "ADDRESS (✓ used)", "BALANCE XMR"],
            &[3, name_w as u16, 22],
        )
    } else {
        (&["#", "ADDRESS (✓ used)", "BALANCE XMR"], &[3, 22])
    };
    table(m, area, title, rows, header, fixed, frame);
}

/// Which columns of the payments table fit the pane it is drawn in (round 4).
///
/// Decided in **one** place so the heading row and every data row build from the same answer. The
/// three that are never dropped are the ones a payment cannot be read without: when, what, and how
/// much. `AMOUNT` keeps 14 cells because `1.234567891234` is 14 and a money value is never cut.
#[derive(Copy, Clone)]
struct HistoryCols {
    confirms: bool,
    locked: bool,
    fee: bool,
    /// How many cells the `NOTE` column gets, or `0` for no column at all (M6).
    note: usize,
}

/// The costs, in order of what a reader gives up last: `WHEN` + `WHAT` + `AMOUNT` is 34 cells,
/// `CONFIRMS` adds 9 and `SPENDABLE` 10 — 53. What is left over goes to `NOTE`, and `FEE`'s fixed
/// 12 comes out of the same remainder.
///
/// **M6: the note comes before the fee, and it takes whatever is left.** It needed a fixed 20
/// cells, which the pane only has at 150 columns — so at the owner's 120 the column vanished
/// entirely, and with it the result of the `n` key, of the `l` key on the pane beside it and of the
/// filter line's own offer to search "by id, address or note". Three features whose effect he could
/// not see. A note is something he wrote himself and this table is the only list of them; a fee is
/// on the payment's own detail view (`Enter`) and in every row `wallet history` prints. Eight cells
/// of a note is a short note read whole or a long one with an ellipsis, and either beats a column
/// that is not there.
fn history_cols(width: u16, wide: bool) -> HistoryCols {
    let w = width as usize;
    let confirms = w >= 43;
    let locked = w >= 53;
    let fixed = 34 + if confirms { 9 } else { 0 } + if locked { 10 } else { 0 };
    let spare = if wide { w.saturating_sub(fixed) } else { 0 };
    // `NOTE` costs a two-cell gap plus at least eight cells; `FEE` costs a two-cell gap plus a
    // fixed ten. Both when there is room for both — which is what 150 columns has — the note
    // alone when there is not, and neither below that.
    let (note, fee) = if spare >= 2 + 8 + 2 + 10 {
        ((spare - 14).min(18), true)
    } else if spare >= 2 + 8 {
        (spare - 2, false)
    } else {
        (0, false)
    };
    HistoryCols {
        confirms,
        locked,
        fee,
        note,
    }
}

struct HistoryItem<'a> {
    row: &'a Transfer,
    th: &'a Theme,
    wh: Option<u64>,
    now: Option<i64>,
    cols: HistoryCols,
}

impl RenderItem for HistoryItem<'_> {
    fn render(&self, area: Rect, frame: &mut Frame, selected: bool, _skip: u16) {
        let (th, t) = (self.th, self.row);
        // Never recomputed here: `history::confirmations` / `history::locked` are the one
        // implementation (§5b), and they take the WALLET's height.
        let conf = history::confirmations(t, self.wh).map_or("—".to_string(), |c| c.to_string());
        let kind = t.kind.as_str();
        let mut text = format!(
            "{:<11} {:<7} {:>14}",
            short_time(t.timestamp),
            // The words, never the wire value: `block` is a mining reward and `pool` is waiting
            // to be mined (§2o rule 5, self-read round 3). The style below still keys on the wire
            // value, which is what `--json` carries.
            t.kind.words(),
            t.amount.to_xmr_string(),
        );
        if self.cols.confirms {
            text.push_str(&format!(" {conf:>8}"));
        }
        if self.cols.locked {
            // Never recomputed here: `history::confirmations` / `history::lock_words` are the one
            // implementation (§5b), and they take the WALLET's height. M29: a word in every state.
            text.push_str(&format!(
                " {:<9}",
                history::lock_words(t, self.wh, self.now)
            ));
        }
        if self.cols.note > 0 {
            let w = self.cols.note;
            text.push_str(&format!("  {:<w$}", fit(&t.note, w)));
        }
        if self.cols.fee {
            // M4: eight decimals, so `0.00000000` and `0.00003072` line up instead of `0.0000`
            // sitting beside `0.00003072` in one column.
            text.push_str(&format!(
                "  {:>10}",
                t.fee.to_xmr_min(fmonero_core::Piconero::FEE_DECIMALS)
            ));
        }
        let style = if selected {
            th.selected()
        } else {
            match kind {
                "in" | "block" => th.ok(),
                "failed" => th.err(),
                "pending" | "pool" => th.warn(),
                _ => th.text(),
            }
        };
        para(text, style, false).render(area, frame);
    }
}

fn history_pane(m: &Tui, area: Rect, frame: &mut Frame, wide: bool) {
    let th = &m.theme;
    let focused = m.wallet.pane == Pane::History;
    let title = format!(
        " Payments  {} · t: {} ",
        m.wallet.rows.len(),
        m.wallet.kind.map_or("all", |k| k.as_str())
    );
    let Some(inner) = framed(area, th, &title, focused, frame) else {
        return;
    };
    if inner.height < 3 {
        return;
    }
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(1), Constraint::Fixed(1), Constraint::Fill])
        .split(inner);
    search_row(
        &m.wallet.filter,
        super::FILTER_HINT,
        m.wallet.filter_open,
        th,
        rows[0],
        frame,
    );
    // Built with the row's own format string, so the header cannot drift out of alignment with
    // its values. `est.` is gone from `LOCKED`: every value in that column is an estimate, and
    // saying so once in the detail modal is enough (§2o).
    // **X20: the unit is on the header of every money column** (§2o rule 9). `AMOUNT` and `FEE`
    // carried none in the cell or the header, on the one table in the program whose numbers are
    // money; the picker's columns got theirs in W20.
    // **X20, and round 4's own read of the captures: the columns are measured against the pane.**
    // `AMOUNT` and `FEE` carried no unit anywhere (rule 9), and simply putting `XMR` on the two
    // headings made the row wider than the pane it is drawn in — the `NOTE` heading vanished and the
    // mining row's fee was cut to `0.00`, a money value clipped mid-number, which is worse than the
    // missing unit it was meant to fix. `history_cols` is the one place the widths are decided, and
    // the header and every row read it, so the two cannot drift apart again.
    let cols = history_cols(inner.width, wide);
    let mut header = format!("{:<11} {:<7} {:>14}", "WHEN", "WHAT", "AMOUNT XMR");
    if cols.confirms {
        header.push_str(&format!(" {:>8}", "CONFIRMS"));
    }
    if cols.locked {
        // **M29: a word in every state.** The column was headed `LOCKED` and left blank for an
        // unlocked payment, with nothing saying that blank meant spendable — and it printed
        // `locked` for a FAILED payment, which never happened and will never unlock. `SPENDABLE`
        // takes `yes`, `not yet`, `never` and `—`, and the reader never has to know what an empty
        // cell was meant to mean.
        header.push_str(&format!(" {:<9}", "SPENDABLE"));
    }
    if cols.note > 0 {
        header.push_str(&format!("  {:<width$}", "NOTE", width = cols.note));
    }
    if cols.fee {
        header.push_str(&format!("  {:>10}", "FEE XMR"));
    }
    para(header, th.dimmed(), false).render(rows[1], frame);
    if let Some(why) = &m.wallet.rows_err {
        para(
            format!("fmonero could not read your payments. {why}"),
            th.err(),
            true,
        )
        .render(rows[2], frame);
        return;
    }
    let shown = m.visible_history();
    if shown.is_empty() {
        // **S5: three empty states, not one sentence for all of them.**
        //
        // One string served an empty wallet, a kind filter that matches nothing and a text filter
        // that matches nothing, so a pane header reading `Payments  6 · t: all` sat above a body
        // reading `Nothing yet. Press R to check` — the header saying six, the body saying none,
        // and the first thing it told him to do the one action that cannot help. Each state names
        // the key that undoes it.
        let filtering = !m.wallet.filter.value().trim().is_empty();
        let text = match (m.wallet.rows.is_empty(), filtering, m.wallet.kind) {
            (true, _, _) => {
                "Nothing yet. Press R to check for payments, or t to change what shows.".to_string()
            }
            (false, true, _) => format!(
                "No payment matches \"{}\". Press / to change it, or Esc to clear it.",
                m.wallet.filter.value().trim()
            ),
            (false, false, Some(k)) => format!(
                "You have {} payments, and none of them is {}. Press t to show them all.",
                m.wallet.rows.len(),
                k.words()
            ),
            // Not reachable — no rows would have matched the first arm — but a screen that
            // renders nothing is worse than a screen that says it does not know.
            (false, false, None) => "Nothing to show.".to_string(),
        };
        para(text, th.dimmed(), true).render(rows[2], frame);
        return;
    }
    let items: Vec<HistoryItem> = shown
        .iter()
        .map(|row| HistoryItem {
            row,
            th,
            wh: m.wallet.wh,
            now: m.wallet.now,
            cols,
        })
        .collect();
    let list = VirtualizedList::new(&items)
        .style(th.text())
        .highlight_style(th.selected())
        .fixed_height(1);
    let mut lst = m.wallet.list.borrow_mut();
    StatefulWidget::render(&list, rows[2], frame, &mut lst);
    m.hits.pane(
        HitPane::History,
        rows[2],
        rows[2].y,
        rows[2].height,
        lst.scroll_offset(),
    );
    drop(lst);
    if m.wallet.wh.is_none() && rows[2].height > 0 {
        // §4.1: `—` rather than 0, and a footer saying which guarantee was given up.
        para(
            "From the saved copy: confirmation counts are not known.".to_string(),
            th.dimmed(),
            false,
        )
        .render(
            Rect::new(rows[2].x, rows[2].bottom() - 1, rows[2].width, 1),
            frame,
        );
    }
}

/// `Enter` on a history row: the facts a user checks after a send (§7a).
fn detail_modal(m: &Tui, frame: &mut Frame) {
    let th = &m.theme;
    let Some(t) = m.selected_transfer() else {
        return;
    };
    // Rows that are only there when they say something (M16, S10): the id row is gone, and the
    // unlock, payment-id and double-spend rows appear only when they carry a fact.
    let h = 11
        + t.destinations.len() as u16
        + u16::from(t.unlock_time > 0)
        + u16::from(!t.payment_id.is_empty())
        + u16::from(t.double_spend_seen);
    let Some(inner) = overlay(m, frame, 76, h, " Payment · n edits the note · Esc closes ")
    else {
        return;
    };
    let bg = th.selection;
    let (dim, text) = (th.dimmed().bg(bg), th.text().bg(bg));
    let pair = |k: &str, v: String| {
        Line::from_spans([Span::styled(format!("{k:<11}"), dim), Span::styled(v, text)])
    };
    let dash = |s: &str| {
        if s.is_empty() {
            "—".to_string()
        } else {
            s.to_string()
        }
    };
    // **No id here** (round 4, S10's rule: ids live in `--json`). Sixty-four hexadecimal
    // characters were the first row of this modal; `wallet history --json` carries every one of
    // them, and `wallet history` prints the payment id a Monero user actually quotes.
    let mut lines = vec![
        pair(
            "what",
            format!(
                "{} · block {} · {}",
                t.kind.words(),
                t.height,
                short_time(t.timestamp)
            ),
        ),
        pair(
            "amount",
            format!(
                "{} XMR · fee {} XMR",
                t.amount.to_xmr_string(),
                t.fee.to_xmr_string()
            ),
        ),
        // M29: `locked true` printed a wire boolean as prose. One wording, from one function,
        // in the table and here.
        pair(
            "confirms",
            format!(
                "{} · spendable {}",
                history::confirmations(t, m.wallet.wh).map_or("—".into(), |c| c.to_string()),
                history::lock_words(t, m.wallet.wh, m.wallet.now)
            ),
        ),
        pair(
            "address #",
            format!(
                "{} (account {})",
                t.subaddr_index.minor, t.subaddr_index.major
            ),
        ),
    ];
    // **Only when there is one** (M16's shape): `unlocks at 0` is a number a reader looks up to
    // learn it means "nothing is set", and it was on every row. Monero's own field is either a
    // block number or a time, and the row says which (§1b.4).
    if t.unlock_time > 0 {
        lines.push(pair(
            "unlocks at",
            if t.unlock_time < fmonero_core::history::MAX_BLOCK_NUMBER {
                format!("block {}", grouped(t.unlock_time))
            } else {
                short_time(t.unlock_time as i64)
            },
        ));
    }
    // A real Monero term where it has a value; an always-present empty row taught it as noise.
    if !t.payment_id.is_empty() {
        lines.push(pair("payment id", t.payment_id.clone()));
    }
    // M16: `double spend false` was a phrase a reader looks up printed on every correct payment.
    // Said only when it is true, and then as the warning it is.
    if t.double_spend_seen {
        lines.push(Line::styled(
            "The network saw these funds spent twice. Do not treat this as paid.",
            th.err().bg(bg),
        ));
    }
    lines.push(pair("note", dash(&t.note)));
    lines.push(Line::styled("destinations", dim));
    if t.destinations.is_empty() {
        lines.push(Line::styled("  none (this payment came in)", dim));
    }
    for d in &t.destinations {
        lines.push(Line::styled(
            format!("  {} {}", d.amount.to_xmr_string(), short_addr(&d.address)),
            text,
        ));
    }
    lines_at(lines, inner, false, frame);
}

// ---------------------------------------------------------------- Log

fn log_screen(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    // The MUTABLE state first, and the path elided to what is left. A `Block` title is clipped
    // at the border width, so a long log path consumed all of it and `follow on/off` — the only
    // persistent indicator of the mode that decides whether the pane scrolls under you — was
    // never on screen at all.
    let on_off = if m.logs.follow { "on" } else { "off" };
    let path = m.logs.path.display().to_string();
    let room = (area.width as usize).saturating_sub(" Log  follow off    ".len());
    let title = format!(
        " Log  follow {on_off}  {} ",
        if path.width() <= room {
            path
        } else {
            fit(&path, room).trim_end().to_string()
        }
    );
    let Some(inner) = framed(area, th, &title, true, frame) else {
        return;
    };
    if inner.height < 2 {
        return;
    }
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(1), Constraint::Fill])
        .split(inner);
    search_row(
        &m.logs.search,
        super::SEARCH_HINT,
        m.logs.search_open,
        th,
        rows[0],
        frame,
    );
    if m.logs.empty {
        // **M8: with a public node there is no node of your own to write one.** This screen
        // promised a log that would never arrive, in the one mode the owner runs — and `L`, the
        // key that changes how much a node logs, is refused in that mode with a message saying
        // exactly that. The screen says it too, and names the way back.
        let text = if m.remote_mode() {
            "This is your own node's log, and you are using a public node. Press 1, then l, to \
             use your own node."
                .to_string()
        } else {
            format!(
                "No log yet. Your node writes it once it runs.\n{}",
                m.logs.path.display()
            )
        };
        para(text, th.dimmed(), true).render(rows[1], frame);
        return;
    }
    StatefulWidget::render(
        &*m.logs.viewer.borrow(),
        rows[1],
        frame,
        &mut m.logs.state.borrow_mut(),
    );
    // The wheel scrolls the log under the cursor (§7a.2). Its rows are not click targets: a log
    // line is not a control, and a click that selected one would do nothing a user asked for.
    m.hits.pane(HitPane::Log, rows[1], rows[1].bottom(), 0, 0);
}

// ---------------------------------------------------------------- modals

/// One modal shape for the wallet picker, the address book, the label/note prompt and the
/// open-wallet password: a bordered overlay with an optional selected list and an optional
/// input row. `sel == usize::MAX` means "nothing selected".
pub(super) fn list_modal(
    m: &Tui,
    title: &str,
    rows: &[String],
    sel: usize,
    input: Option<(&TextInput, &str)>,
    frame: &mut Frame,
) {
    let th = &m.theme;
    // The interior is `h - 4`, and this renders `rows.len() + 1` lines (the rows plus the
    // "Enter · Esc cancels" row) with the input on a row of its own below them. Sizing for
    // `rows.len()` put the input on top of the hint, so the label and note prompts — which pass
    // no rows and a title with no key text — showed an editable field and no keys at all.
    // **X16: a row wider than the modal wraps**, rather than being cut with no marker. The one that
    // did it was the open-a-wallet empty state, whose row is a path: `No wallets in
    // /tmp/…/data/fmonero/w` at both sizes, with the folder's own name the part that was lost.
    const MODAL_W: u16 = 68;
    let text_w = (MODAL_W as usize).saturating_sub(4).max(8);
    let wrapped: Vec<Vec<String>> = rows.iter().map(|r| wrap_words(r, text_w)).collect();
    let drawn: usize = wrapped.iter().map(Vec::len).sum();
    let h = drawn as u16 + 5 + u16::from(input.is_some());
    let Some(inner) = overlay(m, frame, MODAL_W, h, title) else {
        return;
    };
    let bg = th.selection;
    let mut lines: Vec<Line> = Vec::new();
    for (i, parts) in wrapped.into_iter().enumerate() {
        let style = if i == sel {
            th.selected()
        } else {
            th.text().bg(bg)
        };
        for (j, part) in parts.into_iter().enumerate() {
            // A continuation keeps a hanging indent, so a wrapped row cannot read as a second one.
            let text = if j == 0 {
                format!(" {part} ")
            } else {
                format!("   {part} ")
            };
            lines.push(Line::styled(text, style));
        }
    }
    // **X35: `Enter chooses` was said of a free-text field**, on rename, note and add-node alike —
    // and of the open-a-wallet empty state, where there is nothing to choose (X16). Four shapes,
    // one line each, from the arguments this function already has.
    let keys = match (rows.is_empty(), input.is_some(), sel == usize::MAX) {
        // A prompt: one free-text field and nothing else.
        (true, true, _) => "Enter saves · Esc cancels",
        // Rows, and no selection is possible: the empty state.
        (false, _, true) => "Esc closes",
        // Rows plus a field: the wallet list with its password.
        (false, true, false) => "Enter opens · Esc cancels",
        (false, false, false) => "Enter chooses · Esc cancels",
        (true, false, _) => "Esc closes",
    };
    // **M5: §2o rule 11 — a hint is the LAST line.** The note and rename modals rendered this row
    // and then put the only field you can type in *underneath* it, so the instruction came before
    // the thing it was about. The field sits above it now, and the keys close the modal as they do
    // everywhere else.
    let key_row = inner.bottom().saturating_sub(1);
    let input_row = if input.is_some() {
        key_row.saturating_sub(1)
    } else {
        key_row
    };
    lines_at(lines, inner, false, frame);
    para(keys.to_string(), th.dimmed().bg(bg), false)
        .render(Rect::new(inner.x, key_row, inner.width, 1), frame);
    if let Some((input, label)) = input {
        let row = input_row;
        let off = label.width() as u16;
        if off > 0 {
            para(label.to_string(), th.dimmed().bg(bg), false)
                .render(Rect::new(inner.x, row, off, 1), frame);
        }
        input.render(
            Rect::new(
                inner.x + off,
                row,
                inner.width.saturating_sub(off).max(1),
                1,
            ),
            frame,
        );
    }
}

/// A row of `[key] Label` chips, each with a hit rect (§7a.2).
///
/// **The design rule, in one function**: *if a screen tells the user to press a key, that text is
/// the button.* Two affordances, one label, no divergence possible — a hint that is not clickable
/// and a button that does not name its key are the two ways this drifts, and both are impossible
/// when one call renders the text and records the rect.
fn chips(m: &Tui, area: Rect, items: &[(Button, &str, &str)], frame: &mut Frame) {
    let th = &m.theme;
    for (i, (button, key, label)) in items.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.bottom() {
            return;
        }
        let text = format!("[{key}] {label}");
        let w = (text.width() as u16).min(area.width);
        let rect = Rect::new(area.x, y, w, 1);
        para(text, th.selected(), false).render(rect, frame);
        m.hits.button(*button, rect);
    }
}

/// The chips of a key row, laid out horizontally — for the picker's footer, where they sit on one
/// line beside each other rather than stacked.
/// How many rows a chip row needs at this width (X32).
///
/// The picker has six chips and they are 120 columns wide together, so at the owner's own terminal
/// size the fourth one — `[a] Add a node` — and the two after it were simply not drawn: that route
/// had no affordance at all at 120 and one at 150. A chip that does not fit gets the next row.
fn chips_height(items: &[(Button, &str, &str)], width: u16) -> u16 {
    let mut rows = 1u16;
    let mut x = 0u16;
    for (_, key, label) in items {
        let w = format!("[{key}] {label}").width() as u16 + 1;
        if x > 0 && x + w > width {
            rows += 1;
            x = 0;
        }
        x += w;
    }
    rows
}

fn chip_row(m: &Tui, area: Rect, items: &[(Button, &str, &str)], frame: &mut Frame) {
    let th = &m.theme;
    let mut x = area.x;
    let mut y = area.y;
    for (button, key, label) in items {
        let text = format!("[{key}] {label}");
        let w = text.width() as u16 + 1;
        if x > area.x && x + w > area.right() {
            // **X32: the next row, not silently nothing.** A chip is the only affordance its route
            // has; dropping it hides the route.
            y += 1;
            x = area.x;
        }
        if y >= area.bottom() || x + w > area.right() + 1 {
            return;
        }
        let rect = Rect::new(x, y, w - 1, 1);
        para(text, th.selected(), false).render(rect, frame);
        m.hits.button(*button, rect);
        x += w;
    }
}

/// A `◂ value ▸` selector with both arrows as hit rects (§7a.2, revision 6).
///
/// Every control the keymap reaches with `←→` renders its two arrows as targets — the omission
/// that would otherwise have left a mouse-only user unable to change the fee priority, the sending
/// account or the seed language.
fn selector(
    m: &Tui,
    area: Rect,
    which: Selector,
    label: &str,
    value: &str,
    focused: bool,
    frame: &mut Frame,
) {
    let th = &m.theme;
    let style = if focused { th.selected() } else { th.text() };
    let text = format!("{label} ◂ {value} ▸");
    para(text.clone(), style, false).render(area, frame);
    let lead = label.width() as u16 + 1;
    if area.width > lead + 2 {
        m.hits
            .arrow(which, -1, Rect::new(area.x + lead, area.y, 1, 1));
        let tail = text.width() as u16;
        if tail <= area.width {
            m.hits
                .arrow(which, 1, Rect::new(area.x + tail - 1, area.y, 1, 1));
        }
    }
}

/// §7a.5's create-wallet modal: four controls, `Up`/`Down` between them, `Enter` advancing from
/// every one but the last where it submits, and the field key stated **in the modal** because
/// this is one of the exactly two that consume `Tab` (§7a.1, revision 1).
fn create_modal(m: &Tui, form: &super::CreateForm, frame: &mut Frame) {
    let th = &m.theme;
    let (name, pass, again) = (&form.name, &form.pass, &form.again);
    let (lang, field, err) = (form.lang, form.field, form.err.as_deref());
    // **The footer is measured in WRAPPED rows** (round 4's own read of the captures, V15's
    // lesson once more): M23's longer sentence is 76 characters in a 62-cell interior, and it was
    // cut to `Esc closes it an` — a modal whose own explanation of the `Tab` exception did not
    // survive the width the modal is drawn at.
    const CREATE_W: u16 = 66;
    let foot = "Tab cannot change screens while this is open. Esc closes it and keeps nothing.";
    let foot_w = (CREATE_W as usize).saturating_sub(4).max(8);
    let foot_rows = (wrap_words(foot, foot_w).len() as u16).max(1) + 1;
    let Some(inner) = overlay(m, frame, CREATE_W, 11 + foot_rows, " Create a wallet ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    // **Two rows for the footer, not one**: §7a.1 requires the `Tab` exception to be documented
    // *in the modal*, on the line under its controls, and a one-row slot clipped that second line
    // away — leaving the rule true only in this document, which is the drift it exists to prevent.
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fixed(foot_rows),
            Constraint::Fill,
        ])
        .split(inner);
    // W19: `seed lang` was an abbreviation where §7a.5 says `language`, and the four labels are
    // now one style — the Send screen's, which capitalises `To`, `Amount`, `Fee` and `Account`.
    for (i, (label, input)) in [
        // **X34: `Repeat` alone did not say what to repeat**, in the one field a mistake makes a
        // wallet nobody can open — and the four labels are padded to ONE width, so the four input
        // boxes start in one column. `Repeat the password` is five cells longer than `Password`, and
        // left unpadded it moved its own field five cells right of the two above it.
        // **M23: four labels, four nouns.** `Repeat the password` was a verb phrase among three
        // nouns, and the modal's own labels are the one place the four fields are named.
        ("Name                ", name),
        ("Password            ", pass),
        ("Password again      ", again),
    ]
    .iter()
    .enumerate()
    {
        let off = label.width() as u16;
        para((*label).to_string(), th.dimmed().bg(bg), false)
            .render(Rect::new(rows[i].x, rows[i].y, off, 1), frame);
        let box_ = Rect::new(
            rows[i].x + off,
            rows[i].y,
            rows[i].width.saturating_sub(off).max(1),
            1,
        );
        input.render(box_, frame);
        m.hits.field(i, box_);
    }
    selector(
        m,
        rows[3],
        Selector::Language,
        "Seed language       ",
        fmonero_core::settings::SEED_LANGUAGES[lang].1,
        field == 3,
        frame,
    );
    if let Some(why) = err {
        para(why.to_string(), th.err().bg(bg), true).render(rows[4], frame);
    }
    // The one documented `Tab` exception, documented IN the modal (§7a.1).
    // **X34: `Press Esc to discard first.` named no second thing** — first before what? It is Tab
    // that is blocked, so the sentence says what Esc lets you do next. **M23: `first` was left
    // dangling** — X34 replaced one unfinished sentence with another. What Tab normally does is
    // change screens, so that is what the line says it cannot do here, and then what Esc is for.
    let mut foot_lines = vec![Line::styled(
        "↑↓ fields · Enter next, then create · Esc discards",
        th.dimmed().bg(bg),
    )];
    for part in wrap_words(foot, foot_w) {
        foot_lines.push(Line::styled(part, th.warn().bg(bg)));
    }
    lines_at(foot_lines, rows[5], false, frame);
    let _ = rows[6];
}

/// §7a.5's seed-backup modal — dismiss-only, and it renders `seed_backup_hint` verbatim from
/// core. **fmonero never displays a seed**, in the TUI or anywhere else.
fn seed_modal(m: &Tui, lines: &[String], frame: &mut Frame) {
    let th = &m.theme;
    // Sized from the **wrap of its own text**, measured with the same helper that draws it — the
    // hand-written `lines.len() + 6` pushed the last two rows (including "fmonero never displays
    // a seed") off the bottom as soon as the wallet path was long enough to wrap.
    let w = 84.min(frame.bounds().width);
    let wrapped_rows: u16 = lines
        .iter()
        .map(|l| wrap_words(l, w.saturating_sub(4) as usize).len().max(1) as u16)
        .sum();
    let h = wrapped_rows + 7;
    let Some(inner) = overlay(m, frame, w, h, " Back up your seed ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let mut out: Vec<Line> = lines
        .iter()
        .map(|l| Line::styled(l.clone(), th.text().bg(bg)))
        .collect();
    out.push(Line::styled(String::new(), th.text().bg(bg)));
    out.push(Line::styled("Enter to close", th.dimmed().bg(bg)));
    lines_at(out, inner, true, frame);
    let button = Rect::new(inner.x, inner.bottom().saturating_sub(1), 14, 1);
    m.hits.modal_button(button);
}

/// §7a.4's use-this-node confirm — the safety surface of the picker, so it is spelled out: the
/// **full address** (never elided, for the reason the send confirm never elides one), what the
/// node will and will not be told **as a fact rather than a flag** (§2o deletes both the word
/// `untrusted` and the mechanism behind it), then the consequences in order.
fn use_node_modal(m: &Tui, address: &str, wallet_open: Option<&str>, frame: &mut Frame) {
    let th = &m.theme;
    // **M12: what the last check of THIS node found.** The modal listed the address, the trust
    // meaning, the restart and the IP disclosure — and said nothing about the row behind it
    // reading `— │— │— │no answer`, so a user could move his wallet onto a node known not to
    // answer and be told only good things about it. The picker's own row is the source, so the
    // two cannot disagree.
    let checked = m
        .picker_rows()
        .iter()
        .find(|n| n.address == address)
        .and_then(|n| n.probe.as_ref())
        .map(|p| {
            if p.reachable {
                match p.synchronized {
                    Some(true) => {
                        "· when fmonero checked it, it answered and was synced".to_string()
                    }
                    Some(false) => {
                        "· when fmonero checked it, it answered but was still syncing".to_string()
                    }
                    None => "· when fmonero checked it, it answered".to_string(),
                }
            } else {
                format!(
                    "· when fmonero checked it, there was no answer: {}",
                    fmonero_core::nodes::probe_reason_words(p.reason)
                )
            }
        });
    let height = 13 + u16::from(checked.is_some());
    let Some(inner) = overlay(m, frame, 74, height, " Use this node ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let mut lines = vec![
        Line::styled(address.to_string(), th.accented().bg(bg)),
        Line::styled(
            "fmonero will not tell this node which amounts your wallet holds.",
            th.warn().bg(bg),
        ),
        Line::styled(String::new(), th.text().bg(bg)),
    ];
    if let Some(line) = checked {
        let warn = line.contains("no answer");
        lines.push(Line::styled(
            line,
            if warn {
                th.warn().bg(bg)
            } else {
                th.text().bg(bg)
            },
        ));
    } else {
        lines.push(Line::styled(
            "· nothing has checked this node yet. Press p to check the list first",
            th.dimmed().bg(bg),
        ));
    }
    // M10's shape here too: the service restarts only when it is running.
    lines.push(Line::styled(
        if m.status.as_ref().is_some_and(|s| s.wallet.unit_active) {
            "· the wallet service restarts, pointed at this node"
        } else {
            "· the wallet service will use this node when it next starts"
        },
        th.text().bg(bg),
    ));
    if let Some(name) = wallet_open {
        lines.push(Line::styled(
            format!("· {name} will be closed. You will be offered it again."),
            th.text().bg(bg),
        ));
    }
    lines.push(Line::styled(
        "· this node will see your IP address and when you ask it anything",
        th.text().bg(bg),
    ));
    lines.push(Line::styled(String::new(), th.text().bg(bg)));
    lines.push(Line::styled(
        "y to confirm · Esc cancels",
        th.dimmed().bg(bg),
    ));
    lines_at(lines, inner, true, frame);
    m.hits
        .modal_button(Rect::new(inner.x, inner.bottom().saturating_sub(1), 12, 1));
}

/// The `u` and `d` confirms (W2) — **the two keys that start and stop a background service on
/// this machine, and until now the modal said neither.**
///
/// Both were one `Dialog` row reading `y confirms · Esc cancels`, identical to the character in
/// every capture since V4 named them. `u` begins syncing a blockchain into `node.data_dir`, takes
/// up to `timeouts.open`, and keeps going after this window closes; `d` stops the service a wallet
/// gets its blocks from. Same shape as `local_node_modal` and `use_node_modal`: what happens, then
/// the consequences in order, then the keys.
fn node_power_modal(m: &Tui, up: bool, frame: &mut Frame) {
    let th = &m.theme;
    const WIDTH: u16 = 74;
    let text_w = WIDTH.min(frame.bounds().width).saturating_sub(4).max(1) as usize;
    let data_dir = m.ctx.settings.node.data_dir.display().to_string();
    let mut text: Vec<(String, Style)> = Vec::new();
    if up {
        text.push((
            "fmonero starts your node on this machine.".to_string(),
            th.accented(),
        ));
        text.push((String::new(), th.text()));
        text.push((
            format!("· it keeps its copy of the blockchain in {data_dir}"),
            th.text(),
        ));
        text.push((
            format!(
                "· that needs about {} GB of disk, and the first sync takes hours",
                fmonero_core::status::DISK_FLOOR_BYTES / 1_000_000_000
            ),
            th.text(),
        ));
        text.push((
            "· your wallet can only see new payments once the node has them".to_string(),
            th.text(),
        ));
        text.push((fmonero_core::SYNC_KEEPS_GOING.to_string(), th.dimmed()));
    } else {
        // **M10: the confirm describes what will actually happen.** With the node already stopped
        // — which the hero and the badge both said in the same frame — this modal opened with
        // *"fmonero stops your node on this machine"* and then listed the consequences of a stop
        // that cannot occur. The `d` key still reaches this confirm, because a stopped-looking node
        // may simply be one that has not answered yet, and stopping it then is a real act.
        let running = m
            .status
            .as_ref()
            .is_some_and(|s| s.units.node.active || s.node.reachable == Some(true));
        if running {
            text.push((
                "fmonero stops your node on this machine.".to_string(),
                th.accented(),
            ));
            text.push((String::new(), th.text()));
            text.push((
                format!("· nothing is lost. The blockchain stays in {data_dir}"),
                th.text(),
            ));
            text.push((
                "· until it runs again, your wallet cannot see new payments".to_string(),
                th.text(),
            ));
            text.push(("· press u to start it again".to_string(), th.text()));
        } else {
            text.push((
                "Your node is not running, so there is nothing to stop.".to_string(),
                th.accented(),
            ));
            text.push((String::new(), th.text()));
            text.push((
                format!("· nothing is lost either way. The blockchain stays in {data_dir}"),
                th.text(),
            ));
            text.push((
                "· confirming makes sure it stays stopped".to_string(),
                th.text(),
            ));
            text.push(("· press u to start it instead".to_string(), th.text()));
        }
    }
    text.push((String::new(), th.text()));
    text.push(("y to confirm · Esc cancels".to_string(), th.dimmed()));
    // The height is counted in WRAPPED rows, not in sentences: `node.data_dir` is a path of any
    // length and a height counted in sentences leaves the keys under the bottom border (V15).
    let rows: usize = text
        .iter()
        .map(|(t, _)| wrap_bullet(t, text_w).len().max(1))
        .sum();
    let title = if up {
        " Start your node "
    } else {
        " Stop your node "
    };
    let Some(inner) = overlay(m, frame, WIDTH, rows as u16 + 4, title) else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let lines: Vec<Line> = text
        .into_iter()
        .flat_map(|(t, style)| {
            let wrapped = if t.is_empty() {
                vec![String::new()]
            } else {
                wrap_bullet(&t, text_w)
            };
            wrapped
                .into_iter()
                .map(move |l| Line::styled(l, style.bg(bg)))
                .collect::<Vec<_>>()
        })
        .collect();
    lines_at(lines, inner, false, frame);
    m.hits
        .modal_button(Rect::new(inner.x, inner.bottom().saturating_sub(1), 12, 1));
}

/// **X4: how much your node logs, as an overlay rather than a 58-column `Dialog`.**
///
/// The scale was the part nobody had explained, so it gets a line of its own rather than a clause
/// squeezed in beside the keys: 0 is silent, 4 is everything, and the middle is where a person
/// looking for a reason should be.
fn log_level_modal(m: &Tui, level: i8, frame: &mut Frame) {
    let th = &m.theme;
    const WIDTH: u16 = 74;
    let text_w = WIDTH.min(frame.bounds().width).saturating_sub(4).max(1) as usize;
    let mut text: Vec<(String, Style)> = vec![
        (
            format!("Your node will write level {level} to its log file."),
            th.accented(),
        ),
        (String::new(), th.text()),
        ("· 0 is almost silent".to_string(), th.text()),
        (
            "· 1 is the usual amount, and is what fmonero sets up".to_string(),
            th.text(),
        ),
        (
            "· 4 writes everything, and fills the disk fast".to_string(),
            th.text(),
        ),
        (String::new(), th.text()),
        (
            // **S7: `n` does nothing on the Node screen.** The log is screen 4, and `n` is
            // "next match" once you are there — so this line named a key that had no arm at all
            // in `on_node_key`, in the confirm for the setting whose whole purpose is the log.
            "Press 4 to read the log.".to_string(),
            th.text(),
        ),
        (String::new(), th.text()),
        ("y to confirm · Esc cancels".to_string(), th.dimmed()),
    ];
    // The height is counted in WRAPPED rows, never in sentences (V15).
    let rows: usize = text
        .iter()
        .map(|(t, _)| wrap_bullet(t, text_w).len().max(1))
        .sum();
    let Some(inner) = overlay(
        m,
        frame,
        WIDTH,
        rows as u16 + 4,
        " How much your node logs ",
    ) else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let lines: Vec<Line> = std::mem::take(&mut text)
        .into_iter()
        .flat_map(|(t, style)| {
            let wrapped = if t.is_empty() {
                vec![String::new()]
            } else {
                wrap_bullet(&t, text_w)
            };
            wrapped
                .into_iter()
                .map(move |l| Line::styled(l, style.bg(bg)))
                .collect::<Vec<_>>()
        })
        .collect();
    lines_at(lines, inner, false, frame);
    m.hits
        .modal_button(Rect::new(inner.x, inner.bottom().saturating_sub(1), 12, 1));
}

/// §7a.4's `l` confirm — the way back to your own node, and **the same shape as the picker's
/// `Enter` confirm above** (v5.6.1, V4). Three sentences, in the order a user needs them: what
/// restarts, which wallet closes and that it will be offered again, and — when the node is not
/// running — that this switch will not start it, with the key that does.
fn local_node_modal(m: &Tui, frame: &mut Frame) {
    let th = &m.theme;
    let wallet = m
        .status
        .as_ref()
        .filter(|s| s.wallet.open)
        .and_then(|s| s.wallet.name.clone());
    // §7a.4: a modal that implied a running node would be the §2j failure on the one screen whose
    // job is honesty about nodes.
    let node_up = m
        .status
        .as_ref()
        .is_some_and(|s| s.units.node.active || s.node.reachable == Some(true));
    // Every sentence, then the height it needs: the overlay is 74 columns at most and one of these
    // lines is 82 characters, so a height counted in sentences left the last one — the keys —
    // under the bottom border, which is the defect V15 is about happening inside the fix for V4.
    const WIDTH: u16 = 74;
    let text_w = WIDTH.min(frame.bounds().width).saturating_sub(4).max(1) as usize;
    // **M10: the wallet service restarts only if it is running.** With it down the modal
    // promised *"the wallet service will restart"*, which is a promise about something that is
    // not happening. The bullet says what the switch does to the service in either state.
    let wallet_up = m.status.as_ref().is_some_and(|s| s.wallet.unit_active);
    let mut text: Vec<(String, Style)> = vec![
        (
            "fmonero will use the node on this machine.".to_string(),
            th.accented(),
        ),
        (String::new(), th.text()),
        (
            if wallet_up {
                "· the wallet service restarts, pointed at your own node".to_string()
            } else {
                "· the wallet service will use your own node when it next starts".to_string()
            },
            th.text(),
        ),
    ];
    if let Some(name) = wallet {
        text.push((
            format!("· {name} will be closed. You will be offered it again."),
            th.text(),
        ));
    }
    if !node_up {
        // **M10: a bullet, at the bullet indent.** This sentence sat flush with the bullets above
        // it and without one of its own, so it read as a continuation of the line before it.
        text.push((String::new(), th.text()));
        text.push((
            "Your own node is not running yet. Switching will not start it. Press u after this."
                .to_string(),
            th.warn(),
        ));
    }
    text.push((String::new(), th.text()));
    text.push(("y to confirm · Esc cancels".to_string(), th.dimmed()));
    let rows: usize = text
        .iter()
        .map(|(t, _)| wrap_bullet(t, text_w).len().max(1))
        .sum();
    let Some(inner) = overlay(m, frame, WIDTH, rows as u16 + 4, " Use your own node ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let lines: Vec<Line> = text
        .into_iter()
        .flat_map(|(t, style)| {
            let wrapped = if t.is_empty() {
                vec![String::new()]
            } else {
                wrap_bullet(&t, text_w)
            };
            wrapped
                .into_iter()
                .map(move |l| Line::styled(l, style.bg(bg)))
                .collect::<Vec<_>>()
        })
        .collect();
    lines_at(lines, inner, false, frame);
    m.hits
        .modal_button(Rect::new(inner.x, inner.bottom().saturating_sub(1), 12, 1));
}

/// §7a.4's `a` modal: **one** field, validated by the same function the CLI validates with.
///
/// There is deliberately no trusted toggle (revision 5): nothing in the schema supported one,
/// and a path to request it from this screen would make the confirm modal's own sentence an
/// untruth. The footer states the consequence instead of naming a CLI flag (§2o.1).
fn add_node_modal(m: &Tui, input: &TextInput, frame: &mut Frame) {
    let th = &m.theme;
    // **V11: nine rows, not eight.** At eight the interior was four rows, the layout spent one of
    // them on a spacer, and the third line the same call passed in — `Enter adds it · Esc cancels`
    // — was off the bottom at every terminal size. A modal that tells the user neither what is
    // wrong nor which key commits it is two thirds of a modal.
    // **M24's sentence needs the row it wraps onto.** V11 fixed a height counted in sentences
    // once; a longer sentence put `Enter adds it · Esc cancels` back under the bottom border, so the
    // height is counted in WRAPPED rows like every other modal's (V15).
    const WIDTH: u16 = 70;
    let meaning = "fmonero will not tell a node you add here which amounts your wallet holds.";
    let meaning_rows = wrap_words(meaning, (WIDTH as usize).saturating_sub(4).max(8)).len() as u16;
    let Some(inner) = overlay(m, frame, WIDTH, 8 + meaning_rows, " Add a node ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fixed(1),
            Constraint::Fill,
        ])
        .split(inner);
    // **M24: the label is in the same case as every other modal's.** It was `address` in lower
    // case where the create modal has `Name` and `Password`.
    let label = "Address  ";
    let off = label.width() as u16;
    para(label.to_string(), th.dimmed().bg(bg), false)
        .render(Rect::new(rows[0].x, rows[0].y, off, 1), frame);
    let box_ = Rect::new(
        rows[0].x + off,
        rows[0].y,
        rows[0].width.saturating_sub(off).max(1),
        1,
    );
    input.render(box_, frame);
    m.hits.field(0, box_);
    // **V11: validated live by the same function the CLI validates with** (§7a.4, §5b), so the
    // TUI cannot accept a string `fmonero node remote` refuses and the user is told what is wrong
    // while they type rather than after they press Enter. An empty field is not an error — it is
    // the state the modal opens in — so it gets the example instead.
    let typed = input.value().trim();
    let (verdict, style) = if typed.is_empty() {
        (
            "host:port, like node.example.com:18089".to_string(),
            th.dimmed().bg(bg),
        )
    } else {
        match fmonero_core::settings::Settings::parse_remote_address(typed) {
            Ok(addr) => (format!("{addr} · press Enter to add it"), th.ok().bg(bg)),
            Err(e) => (e.to_string(), th.err().bg(bg)),
        }
    };
    lines_at(vec![Line::styled(verdict, style)], rows[1], true, frame);
    lines_at(
        vec![
            // **M24: what it means for him, not what fmonero does internally.** `Nodes you add
            // here are treated as public.` describes a classification; the node-switch confirm
            // two keys away already says the same fact the right way round.
            Line::styled(meaning.to_string(), th.dimmed().bg(bg)),
            Line::styled(
                "Enter adds it · Esc cancels".to_string(),
                th.dimmed().bg(bg),
            ),
        ],
        rows[3],
        true,
        frame,
    );
    let _ = rows[2];
}

/// §2n.5 rule 2's notice, shown before the first fetch **or** the first probe while the cache's
/// `notice_ack` is unset — and `Enter` writes that field, which is the consent record.
fn notice_modal(m: &Tui, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = overlay(m, frame, 72, 10, " Before contacting anyone ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    lines_at(
        vec![
            Line::styled(
                "Getting the list tells monero.fail your IP address.",
                th.text().bg(bg),
            ),
            Line::styled(
                "Checking a node tells that node too. Nothing else is contacted.",
                th.text().bg(bg),
            ),
            Line::styled(String::new(), th.text().bg(bg)),
            Line::styled(
                "monero.fail's figures are its own measurements. They are not a recommendation, \
                 and not yours.",
                th.dimmed().bg(bg),
            ),
            Line::styled(String::new(), th.text().bg(bg)),
            Line::styled("Enter continues · Esc cancels", th.dimmed().bg(bg)),
        ],
        inner,
        true,
        frame,
    );
    m.hits
        .modal_button(Rect::new(inner.x, inner.bottom().saturating_sub(1), 16, 1));
}

/// The receive QR: the address painted onto an `ftui_extras` `Canvas` in **half-block** mode,
/// one module per sub-pixel — one terminal column wide and half a row tall, so the code comes
/// out square (§7a). A terminal too small for it says so and still shows the address.
fn qr_modal(m: &Tui, address: &str, frame: &mut Frame) {
    let th = &m.theme;
    let Ok(code) = QrCode::new(address.as_bytes()) else {
        return;
    };
    // **M30: which address this is, and what to do with it.** The modal was a code and 95
    // characters, with no account number, no `#N` and nothing saying that showing it is how you
    // get paid — on the one screen whose whole purpose is handing an address to somebody else.
    let which = m
        .wallet
        .addresses
        .as_ref()
        .and_then(|set| set.addresses.iter().find(|a| a.address == address))
        .map(|a| {
            if a.label.trim().is_empty() {
                format!("Your address #{}", a.address_index)
            } else {
                format!("Your address #{} · {}", a.address_index, a.label)
            }
        })
        .unwrap_or_else(|| "Your main address".to_string());
    let quiet = 2u16;
    let modules = code.width() as u16;
    let side = modules + quiet * 2;
    let qr_rows = side.div_ceil(2);
    // `overlay` spends a row and a column on its border and one more of each on its padding,
    // so the interior it hands back is (w - 4, h - 4): asking for exactly the code's own
    // height left the address nowhere to go and reported a 120x40 terminal as too small.
    let text_rows = wrapped(address, side as usize).len() as u16;
    // Two more rows: the heading above the code and the sentence under the address.
    let Some(inner) = overlay(
        m,
        frame,
        side + 4,
        qr_rows + text_rows + 6,
        " Receive · Esc closes ",
    ) else {
        return;
    };
    para(which, th.accented().bg(th.selection), false)
        .render(Rect::new(inner.x, inner.y, inner.width, 1), frame);
    let inner = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let fits = inner.height >= qr_rows && inner.width >= side;
    if fits {
        let colors = code.to_colors();
        let area = Rect::new(inner.x, inner.y, side, qr_rows);
        // **Every sub-pixel gets an explicit colour, and the paper is painted first.**
        // `Painter::halfblock_cell` returns `('▀', top_color, bot_color)` for a cell whose two
        // sub-rows are both lit, and an uncoloured `point()` leaves both `None`
        // (`ftui-extras-0.7.0/src/canvas.rs:989-1005`, `:251-263`) — so the base style's `bg`
        // painted the lower half of every such cell in the OTHER colour. Lighting the light
        // modules that way put a dark stripe through every light module pair, the quiet zone
        // included, and no decoder locks on to that. It also skips `' '` cells entirely, so the
        // modules it left unpainted showed the overlay's own background rather than QR black.
        //
        // So: lay down solid QR-light paper, then draw the DARK modules as coloured sub-pixels
        // with `th.qr()` = dark ink on light paper. Both-dark gives fg=bg=dark (a solid dark
        // cell), one-dark gives the right half-block, both-light is skipped and stays paper.
        let paper = Text::from_lines(
            (0..qr_rows).map(|_| Line::styled(" ".repeat(side as usize), th.qr())),
        );
        Paragraph::new(paper)
            .wrap(WrapMode::None)
            .render(area, frame);
        let mut painter = Painter::new(side, side, Mode::HalfBlock);
        let ink = th.qr().fg.unwrap_or_default();
        for y in 0..side {
            for x in 0..side {
                let inside = x >= quiet && y >= quiet && x < quiet + modules && y < quiet + modules;
                let dark = inside
                    && colors[(y - quiet) as usize * modules as usize + (x - quiet) as usize]
                        == QrColor::Dark;
                if dark {
                    painter.point_colored(i32::from(x), i32::from(y), ink);
                }
            }
        }
        Canvas::from_painter(&painter)
            .style(th.qr())
            .render(area, frame);
    } else {
        para(
            "Window too small for a QR code. The address is below.".to_string(),
            th.warn(),
            true,
        )
        .render(
            Rect::new(inner.x, inner.y, inner.width, inner.height.min(2)),
            frame,
        );
    }
    let used = if fits { qr_rows } else { 2 };
    let rest = Rect::new(
        inner.x,
        inner.y + used.min(inner.height),
        inner.width,
        inner.height.saturating_sub(used),
    );
    if rest.height > 0 {
        // The address is never elided: it is what the sender will copy.
        let mut lines: Vec<Line> = wrapped(address, rest.width as usize)
            .into_iter()
            .map(|l| Line::styled(l, th.text().bg(th.selection)))
            .collect();
        // M30: what the code is for. Money arriving at an address nobody was given is the one
        // thing this modal cannot produce.
        lines.push(Line::styled(
            "Show this, or send the line above, to be paid.",
            th.dimmed().bg(th.selection),
        ));
        lines_at(lines, rest, false, frame);
    }
}

/// The plain confirm modal — node up/down and the log level. `y` activates it, as §7a asks.
/// The `?` overlay (§7a: **it is the keymap**) — **wrapped, never elided** (W13).
///
/// V15 gave every cut line an `…`, which made the overlay honest and still unreadable: 11 of its
/// 25 rows ended in one at 120 columns and 9 at 150, including the two that explain why `u`, `d`
/// and `L` are unavailable and what `Esc` does. A key's line is the only place those facts exist,
/// so the fix is room: the overlay is as wide as the frame allows up to 118 columns, the key
/// column is 19, and an action that still does not fit continues on a second row indented under
/// itself. Nothing here may end in `…`, and `tests.rs` asserts exactly that.
fn help(m: &Tui, frame: &mut Frame) {
    let th = &m.theme;
    /// The key column, measured: `Send: p  Ctrl+P  @` is the widest at 18 cells.
    const KEY_W: usize = 19;
    const WIDTH: u16 = 118;
    let w = WIDTH.min(frame.bounds().width);
    let text_w = (w as usize).saturating_sub(4).saturating_sub(KEY_W).max(8);
    // §7a: in remote mode the keys that need the local daemon are marked from this same table,
    // so help and behaviour cannot drift — `confirm` toasts, and `b` has no bans to show.
    let remote = m.remote_mode();
    let mut rows: Vec<(String, String, bool)> = Vec::new();
    for (k, a) in KEYS {
        let disabled = remote && matches!(*k, "u  d" | "L" | "b");
        // The mouse row reads its own setting rather than a copy of it, so the overlay's text
        // cannot drift from the policy actually in force (§7a.2's reason for making it a
        // setting and not a flag).
        let action = if disabled {
            // **X28: two parentheticals in a row.** `L  Node: change how much your node logs (it
            // asks first) (not while you use a public node)` — the same shape the self-read fixed
            // for `version_supported`. The key's own parenthetical goes when this one applies: it
            // cannot ask first if it cannot run at all.
            let stem = match a.split_once(" (") {
                Some((head, _)) => head,
                None => *a,
            };
            format!("{stem}. Not while you use a public node.")
        } else if *k == "mouse" && !m.ctx.settings.tui.mouse {
            // The settings key stays: it is a name the user types, which is §2o rule 5's
            // own carve-out. What goes is the shell command (§2o.1).
            "off. Turn on tui.mouse in your settings".to_string()
        } else {
            (*a).to_string()
        };
        rows.push(((*k).to_string(), action, disabled));
    }
    let wrapped: Vec<(String, Vec<String>, bool)> = rows
        .into_iter()
        .map(|(k, a, d)| {
            let lines = wrap_words(&a, text_w);
            (k, lines, d)
        })
        .collect();
    let height: usize = wrapped.iter().map(|(_, l, _)| l.len().max(1)).sum();
    let Some(inner) = overlay(m, frame, w, height as u16 + 4, " Keys · any key closes ") else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    for (k, action, disabled) in wrapped {
        let style = if disabled {
            th.dimmed().bg(th.selection)
        } else {
            th.text().bg(th.selection)
        };
        for (i, part) in action.iter().enumerate() {
            let key = if i == 0 { k.clone() } else { String::new() };
            lines.push(Line::from_spans([
                Span::styled(crate::output::pad(&key, KEY_W), th.key().bg(th.selection)),
                Span::styled(part.clone(), style),
            ]));
        }
    }
    lines_at(lines, inner, false, frame);
}

// ---------------------------------------------------------------- drawing helpers

fn para(text: impl Into<String>, style: Style, wrap: bool) -> Paragraph<'static> {
    Paragraph::new(text.into()).style(style).wrap(if wrap {
        WrapMode::Word
    } else {
        WrapMode::None
    })
}

fn line_at(spans: Vec<Span<'static>>, area: Rect, frame: &mut Frame) {
    Paragraph::new(Text::from_line(Line::from_spans(spans)))
        .wrap(WrapMode::None)
        .render(area, frame);
}

fn lines_at(lines: Vec<Line<'static>>, area: Rect, wrap: bool, frame: &mut Frame) {
    Paragraph::new(Text::from_lines(lines))
        .wrap(if wrap { WrapMode::Word } else { WrapMode::None })
        .render(area, frame);
}

/// `label      value` — the one row shape every panel uses.
/// The first letter of a phrase, for the one place a vocabulary word starts a sentence (M3).
///
/// The words themselves live in `nodes::probe_reason_words` in the form a table cell wants, so a
/// second table of capitalised copies is a second table to drift.
fn sentence_case(words: &str) -> String {
    let mut c = words.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn kv(th: &Theme, label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from_spans([
        // 11 wide **plus one space**: `speed limits` is twelve characters and came out as
        // `speed limits↑2048` — a number touching a word is a number nobody can read.
        Span::styled(format!("{label:<11} "), th.dimmed()),
        Span::styled(value.into(), th.text()),
    ])
}

pub(super) fn panel<'a>(th: &Theme, title: &'a str, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            th.border_focused()
        } else {
            th.border()
        })
        .title(title)
        .padding(Sides::horizontal(1))
}

/// Draw a bordered panel and return its interior, or `None` when there is no room for one.
pub(super) fn framed(
    area: Rect,
    th: &Theme,
    title: &str,
    focused: bool,
    frame: &mut Frame,
) -> Option<Rect> {
    if area.height < 3 || area.width < 8 {
        return None;
    }
    let block = panel(th, title, focused);
    let inner = block.inner(area);
    block.render(area, frame);
    (inner.height > 0).then_some(inner)
}

/// A centred modal: blank its area, draw its border, return the interior.
pub(super) fn overlay(m: &Tui, frame: &mut Frame, w: u16, h: u16, title: &str) -> Option<Rect> {
    let th = &m.theme;
    let b = frame.bounds();
    let (w, h) = (w.min(b.width), h.min(b.height));
    if w < 20 || h < 4 {
        return None;
    }
    let area = Rect::new(b.x + (b.width - w) / 2, b.y + (b.height - h) / 2, w, h);
    let blank = Text::from_lines((0..area.height).map(|_| {
        Line::styled(
            " ".repeat(area.width as usize),
            Style::new().bg(th.selection),
        )
    }));
    Paragraph::new(blank)
        .wrap(WrapMode::None)
        .render(area, frame);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(th.border_focused())
        .title(title)
        .padding(Sides::all(1))
        .style(Style::new().bg(th.selection));
    let inner = block.inner(area);
    block.render(area, frame);
    (inner.height > 0).then_some(inner)
}

fn table(
    m: &Tui,
    area: Rect,
    title: &str,
    rows: Vec<TableRow>,
    header: &[&'static str],
    fixed: &[u16],
    frame: &mut Frame,
) {
    let th = &m.theme;
    let mut widths: Vec<Constraint> = fixed.iter().map(|w| Constraint::Fixed(*w)).collect();
    while widths.len() + 1 < header.len() {
        widths.push(Constraint::Fill);
    }
    widths.push(Constraint::Fill);
    let t = Table::new(rows, widths)
        .header(TableRow::new(header.to_vec()).style(th.dimmed()))
        .block(panel(th, title, false))
        .style(th.text())
        .highlight_style(th.selected());
    let (state, pane) = if m.screen == Screen::Wallet {
        (&m.wallet.table, HitPane::Subaddresses)
    } else {
        (&m.node.table, HitPane::NodeList)
    };
    let mut st = state.borrow_mut();
    StatefulWidget::render(&t, area, frame, &mut st);
    // §7a.2: the geometry, so `hit.rs` can turn a y back into a row. `+2` skips the panel's top
    // border and the header row; the last row is inside the bottom border.
    if area.height > 3 {
        m.hits.pane(
            pane,
            area,
            area.y + 2,
            area.height.saturating_sub(3),
            st.offset,
        );
    }
}

fn search_row(
    input: &TextInput,
    hint: &str,
    open: bool,
    th: &Theme,
    area: Rect,
    frame: &mut Frame,
) {
    let style = if open { th.accented() } else { th.dimmed() };
    para("/ ".to_string(), style, false).render(area, frame);
    hinted_input(
        th,
        input,
        hint,
        Rect::new(area.x + 2, area.y, area.width.saturating_sub(2).max(1), 1),
        frame,
    );
}

/// A `TextInput` plus its own dim hint while it is empty.
///
/// ftui's own empty-field text is set through a builder whose name is one of the words
/// `scripts/check.sh`'s §2h grep forbids in shipped source, and it would appear on every
/// input in the TUI — a grep that fires on correct code is the grep an agent relaxes (§2j).
/// So the hint is drawn here instead: the input renders first (it owns the cursor), the hint
/// over its empty cells second.
pub(super) fn hinted_input(
    th: &Theme,
    input: &TextInput,
    hint: &str,
    area: Rect,
    frame: &mut Frame,
) {
    input.render(area, frame);
    if input.value().is_empty() && !hint.is_empty() {
        para(hint.to_string(), th.dimmed(), false).render(area, frame);
    }
}

// ---------------------------------------------------------------- formatting

/// Truncate to a display width, padding to it, with an ellipsis when it does not fit.
pub(super) fn fit(s: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    if s.width() <= n {
        return format!("{s:<n$}");
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + 1 >= n {
            break;
        }
        out.push(c);
    }
    out.push('…');
    format!("{out:<n$}")
}

// **`elide` is gone** (W13, W14). It existed for two callers — the status bar's toast and the `?`
// overlay's rows — and both now have room instead of a mark: a toast that does not fit beside the
// badges gets a row of its own and wraps, and the overlay wraps its rows. The honest ellipsis
// V15 added was still the reader losing the end of the sentence. `fit` remains, for table CELLS,
// where a column has a fixed width and a value that overruns it is a value, not prose.

/// Hard-wrap at a width, for the one string that may never be elided: an address.
pub(super) fn wrapped(s: &str, width: usize) -> Vec<String> {
    if width == 0 || s.is_empty() {
        return vec![String::new()];
    }
    s.chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|c| c.iter().collect())
        .collect()
}

/// Word-wrap prose at a width, for the lines that carry a **runnable command**: a clipped
/// `fmonero setup --units` reads as the complete and plausible `fmonero setup --unit`, which
/// exits 2. `wrapped` hard-chunks and is for addresses; this breaks on spaces.
/// [`wrap_words`], but a bullet's continuation lines are indented under its text (round 4's own
/// read of the captures).
///
/// `· nothing is lost either way. The blockchain stays in /tmp/…/bitmonero` wrapped in the Stop
/// confirm and the path landed flush with the modal's border, reading as a line of its own rather
/// than as the rest of the bullet. Two spaces is exactly the width of `· `.
pub(super) fn wrap_bullet(s: &str, width: usize) -> Vec<String> {
    if !s.starts_with("· ") {
        return wrap_words(s, width);
    }
    let mut out = wrap_words(s, width.max(3));
    for line in out.iter_mut().skip(1) {
        *line = format!("  {line}");
    }
    out
}

pub(super) fn wrap_words(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.width() + 1 + word.width() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
        // A single word longer than the width still has to go somewhere: hard-chunk that one.
        while line.width() > width {
            let head: String = line.chars().take(width).collect();
            let tail: String = line.chars().skip(width).collect();
            out.push(head);
            line = tail;
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// For tables and labels only, never for a recipient being confirmed.
pub(super) fn short_addr(a: &str) -> String {
    let n = a.chars().count();
    if n <= 20 {
        return a.to_string();
    }
    let head: String = a.chars().take(8).collect();
    let tail: String = a.chars().skip(n - 8).collect();
    format!("{head}…{tail}")
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or("—".to_string(), |x| x.to_string())
}

/// A size, from the one formatter the CLI and the widget also read (X6).
///
/// This divided by **1024** and labelled the result `kB`, so it was wrong about its own divisor and
/// disagreed with both other surfaces: the same waiting pool read `21.4 kB` in `node info` and
/// `20.9 kB` here.
pub(super) use fmonero_core::bytes;
use fmonero_core::bytes_per_sec as rate;

/// How long ago, in one wording for the whole program (W12). The implementation is
/// [`fmonero_core::age`]: the doctor divided seconds by 60 itself and printed `342 min ago` for
/// a list this screen called `5h 42m ago`.
pub(super) use fmonero_core::age;

/// A unix second as `MM-DD HH:MM`; `0` (a pending row) is `—`.
pub(super) fn short_time(unix: i64) -> String {
    match fmonero_core::rfc3339_from_unix(unix).filter(|_| unix > 0) {
        Some(s) if s.len() >= 16 => format!("{} {}", &s[5..10], &s[11..16]),
        _ => "—           ".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resize_tiers_are_the_ones_the_design_names() {
        assert_eq!(tier_of(Rect::new(0, 0, 120, 40)), Tier::Wide);
        assert_eq!(tier_of(Rect::new(0, 0, 100, 20)), Tier::Wide);
        assert_eq!(tier_of(Rect::new(0, 0, 99, 20)), Tier::Medium);
        assert_eq!(tier_of(Rect::new(0, 0, 60, 20)), Tier::Medium);
        assert_eq!(tier_of(Rect::new(0, 0, 59, 40)), Tier::Narrow);
        assert_eq!(tier_of(Rect::new(0, 0, 120, 19)), Tier::Narrow);
        assert_eq!(tier_of(Rect::new(0, 0, 39, 40)), Tier::Tiny);
        assert_eq!(tier_of(Rect::new(0, 0, 120, 9)), Tier::Tiny);
    }

    #[test]
    fn an_address_wraps_rather_than_eliding_when_it_is_being_confirmed() {
        let a = "4".repeat(95);
        let lines = wrapped(&a, 40);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.concat(), a, "no character may be lost");
        assert!(!lines.concat().contains('…'));
        assert_eq!(wrapped("", 10), vec![String::new()]);
    }

    #[test]
    fn human_numbers_and_short_forms_read_like_the_cli_ones() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(2048), "2.0 kB");
        assert_eq!(age(3725), "1h 02m ago");
        assert_eq!(fit("CH", 5), "CH   ");
        assert_eq!(fit("a long label", 6).width(), 6);
        assert_eq!(short_time(0).trim(), "—");
        assert_eq!(short_addr("short"), "short");
        let a = "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm";
        assert!(short_addr(a).starts_with("42ey1afD"));
        assert!(short_addr(a).ends_with("JJQAWDm"));
        assert_eq!(opt::<u64>(None), "—");
    }
}
