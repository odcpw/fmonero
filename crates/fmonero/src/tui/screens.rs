//! Every cell the TUI draws (DESIGN.md §7a): the chrome, the four screens, the modals, the
//! help overlay and the four resize tiers. `view` returns on a zero area and never leaves a
//! screen blank — each empty, error and hang state has its own text.

use ftui::layout::{Constraint, Flex, Rect, Sides};
use ftui::text::wrap::WrapMode;
use ftui::text::{Line, Span, Text};
use ftui::widgets::block::Block;
use ftui::widgets::borders::BorderType;
use ftui::widgets::input::TextInput;
use ftui::widgets::modal::Dialog;
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
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(1),
            Constraint::Fixed(banner),
            Constraint::Fill,
            Constraint::Fixed(1),
        ])
        .split(b);
    tab_strip(m, rows[0], frame, tier);
    if banner > 0 {
        pending_banner(m, rows[1], frame);
    }
    match m.screen {
        Screen::Node => node_screen(m, rows[2], frame, tier),
        Screen::Wallet => wallet_screen(m, rows[2], frame, tier),
        Screen::Send => super::send::view(m, rows[2], frame, tier),
        Screen::Log => log_screen(m, rows[2], frame),
    }
    status_line(m, rows[3], frame);
    match &m.modal {
        Some(Modal::Confirm { dialog, state, .. }) => {
            StatefulWidget::render(dialog.as_ref(), b, frame, &mut state.borrow_mut());
        }
        Some(Modal::OpenWallet {
            names,
            sel,
            pass,
            asking,
        }) => {
            let rows: Vec<String> = if names.is_empty() {
                // §2o.1: one key creates a wallet, so the empty state names the key, not the
                // command it would have printed into a modal nobody can type into.
                vec![
                    format!("No wallets in {}.", m.ctx.settings.wallet.dir.display()),
                    "Press c to create one.".into(),
                ]
            } else {
                names.clone()
            };
            list_modal(
                m,
                " Open a wallet — Enter opens · Esc cancels ",
                &rows,
                if names.is_empty() { usize::MAX } else { *sel },
                asking.then_some((pass, "password ")),
                frame,
            );
        }
        Some(Modal::Prompt { title, input, .. }) => {
            list_modal(m, title, &[], usize::MAX, Some((input, "")), frame);
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
    if let Some(net) = m.status.as_ref().and_then(|s| s.node.nettype.clone()) {
        right.push_str(&format!("{net} "));
    }
    right.push_str(&th.name);
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
    let txid: String = p
        .txs
        .first()
        .map_or("—".into(), |t| t.txid.chars().take(8).collect());
    format!(
        "SENDING {} XMR to {} · {} of {} sent · id {txid} · press 2 for the wallet",
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
        if let Some(h) = s.node.height {
            spans.push(Span::styled(format!("{h} "), th.dimmed()));
        }
        if let Some(out) = s.node.outgoing_connections_count {
            let inc = s.node.incoming_connections_count.unwrap_or(0);
            spans.push(Span::styled(format!("{out}↑{inc}↓ "), th.dimmed()));
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
    if !m.note.is_empty() {
        spans.push(Span::styled("│ ", th.dimmed()));
        spans.push(Span::styled(
            m.note.clone(),
            if m.note_err { th.err() } else { th.ok() },
        ));
    }
    line_at(
        spans,
        Rect::new(area.x, area.y, area.width.saturating_sub(19), 1),
        frame,
    );
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
pub(super) fn probe_reason(reason: Option<&'static str>) -> &'static str {
    match reason {
        Some("timed_out") => "too slow",
        Some("refused") => "refused",
        Some("bad_json") => "bad answer",
        Some("cancelled") => "stopped",
        Some(_) => "no answer",
        None => "no answer",
    }
}

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
        chips(
            m,
            rows[1],
            &[(Button::RefreshList, "f", "Get the list")],
            frame,
        );
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

    if health {
        header.push("reliability");
        header.push("block");
    }
    if checked {
        header.push("last seen");
    }
    if probe_cols {
        header.push("ping");
        header.push("block");
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
            let tag = if n.custom { " yours" } else { "" };
            let mut cells = vec![format!("{mark}{}{tag}", n.address)];
            if health {
                // The ratio **with its sample size**: 100 % of 3 and 72 % of 94 are not
                // comparable, and a bare percent invites the comparison (§2n.1). A `custom` row
                // renders `—` here because monero.fail has never heard of it.
                cells.push(match (n.health_ratio, n.health_checks) {
                    (Some(r), Some(c)) => {
                        format!("{}% of {c} checks", (r * 100.0).round() as i64)
                    }
                    _ => "—".into(),
                });
                cells.push(
                    n.health_last_height
                        .map(|h| h.to_string())
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
                        cells.push(opt(p.latency_ms));
                        cells.push(opt(p.height));
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
    let rows = Flex::vertical()
        .constraints([Constraint::Fill, Constraint::Fixed(1), Constraint::Fixed(1)])
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
    // The footer, which is where `skipped_no_port` and `skipped_invalid` surface — a list that
    // silently showed 158 of 195 would look like a truncated fetch (§2n.1).
    if let Some(list) = m.picker.list.as_ref() {
        let footer = match &m.picker.progress {
            Some(p) => p.clone(),
            // One total, not four numbers, two of which named parse failures: both skip
            // counts stay in `node list --json`, which is where a machine reads them (§2o).
            None => format!(
                "{} nodes from monero.fail · list {} old · {} skipped",
                rows_data.len(),
                list.cache_age_sec.map(age).unwrap_or_else(|| "—".into()),
                list.skipped_no_port + list.skipped_invalid,
            ),
        };
        para(
            fit(&footer, rows[1].width as usize),
            if m.picker.progress.is_some() {
                th.warn()
            } else {
                th.dimmed()
            },
            false,
        )
        .render(rows[1], frame);
    }
    chip_row(
        m,
        rows[2],
        &[
            (Button::Probe, "p", "Check these"),
            (
                Button::RefreshList,
                "f",
                if m.picker.list.is_some() {
                    "Get a fresh list"
                } else {
                    "Get the list"
                },
            ),
            (Button::UseNode, "Enter", "Use this node"),
            (Button::AddNode, "a", "Add a node"),
            (Button::PrevNode, "H", "Back to the last one"),
            // v5.6, §2o.1: the destination the old refusal named as a shell command.
            (Button::UseLocal, "l", "Use your own node"),
        ],
        frame,
    );
}

fn node_hero(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Node ", m.status_err.is_none(), frame) else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    let mut gauge = None;
    if let Some(e) = &m.status_err {
        lines.push(Line::styled(
            format!("Could not read the status: {e}"),
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
                lines.push(kv(
                    th,
                    "this wallet",
                    format!(
                        "block {} · checked {}",
                        opt(s.wallet.height),
                        s.wallet
                            .last_refresh_at
                            .clone()
                            .unwrap_or_else(|| "—".into())
                    ),
                ));
                // Decision 7, on the screen a user watches while it happens: the scanning is
                // inside the wallet service, so closing this window does not stop it.
                if let Some(line) = wallet_sync_line(s) {
                    lines.push(Line::styled(line, th.warn()));
                    lines.push(Line::styled(
                        "Syncing in the background. You can close this window.",
                        th.dimmed(),
                    ));
                }
            }
            lines_at(lines, inner, false, frame);
            return;
        }
        if n.reachable != Some(true) {
            // Word-wrapped, not clipped: the hero is rendered `WrapMode::None` for its `kv`
            // rows, and at 150 columns this 75-character line came out as
            // `fmonero setup --unit` — a command that looks complete and exits 2.
            let text = if m.monero_missing() {
                "Monero is not installed on this machine.  In a terminal: omarchy pkg add monero"
            } else {
                "Your node is not running. Press u to start it."
            };
            lines.extend(
                wrap_words(text, inner.width as usize)
                    .into_iter()
                    .map(|l| Line::styled(l, th.warn())),
            );
        }
        if let Some(pct) = n.sync_percent {
            let target = match (n.target_height, n.height) {
                (Some(0), Some(h)) => h.to_string(),
                (Some(t), _) => t.to_string(),
                _ => "—".into(),
            };
            gauge = Some((lines.len() as u16, pct / 100.0));
            lines.push(kv(
                th,
                "blocks",
                format!("{:13}{pct:.0}%  {} / {target}", "", opt(n.height)),
            ));
            if let Some(eta) = m.sync_eta() {
                lines.push(kv(th, "done in", eta));
            }
        }
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

/// A block number as a person reads it: `3 765 346`, thin spaces every three digits, the same
/// grouping the bar widget's `Model.grouped` applies (§2o rule 9).
pub(super) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Decision 7's line, on both screens that render it and in the widget, from one formula.
///
/// `None` when the wallet is not behind, or when either height is unknown — a percentage
/// computed from a missing node height would be the rendered untruth §2j forbids. The node's
/// height is the live `get_info` with your own node and the **cached check's** with a public
/// one, which is the only source there is (§5's `node.remote_probe`).
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
            lines.push(Line::styled(
                format!("No answer when checked {}.", age(p.age_sec.max(0) as u64)),
                th.warn(),
            ));
            (Button::RefreshList, "f", "Choose another node")
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
            (Button::RefreshList, "f", "Choose another node")
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
    let title = if bans {
        format!(" Bans  {} · b: peers ", extras.map_or(0, |e| e.bans.len()))
    } else {
        format!(
            " Peers  {} · b: bans ",
            extras.map_or(0, |e| e.connections.len())
        )
    };
    let empty = |text: &str, frame: &mut Frame| {
        if let Some(inner) = framed(area, th, &title, false, frame) {
            para(text.to_string(), th.dimmed(), true).render(inner, frame);
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
        let text = m
            .node
            .err
            .clone()
            .unwrap_or_else(|| "checking the node…".into());
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
            let mut cells = vec![fit(&c.host, 22), c.state.clone(), c.height.to_string()];
            if wide {
                cells.push(c.pruning_seed.to_string());
                cells.push(format!("{}/{}", bytes(c.avg_download), bytes(c.avg_upload)));
            }
            TableRow::new(cells)
        })
        .collect();
    let (header, widths): (&[&str], &[u16]) = if wide {
        (
            &["HOST", "STATE", "BLOCK", "KEEPS", "AVG ↓/↑"],
            &[23, 14, 9, 6],
        )
    } else {
        (&["HOST", "STATE", "BLOCK"], &[23, 14])
    };
    table(m, area, &title, rows, header, widths, frame);
}

/// **Never rendered with a public node** (§7a.3, v5.6): v5.5's version of this pane was a
/// frame around the sentence *"not available with a remote node"*, and `node_screen` now lays
/// out ` This node ` there instead.
fn net_pane(m: &Tui, area: Rect, frame: &mut Frame, sparkline: bool) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Net ", false, frame) else {
        return;
    };
    let Some(n) = m.status.as_ref().map(|s| &s.node.net) else {
        para("Your node is not answering.", th.dimmed(), false).render(inner, frame);
        return;
    };
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(2), Constraint::Fill])
        .split(inner);
    let bps = |v: Option<u64>| v.map_or("—".to_string(), rate);
    lines_at(
        vec![
            kv(th, "↓ ↑", format!("{}   {}", bps(n.rx_bps), bps(n.tx_bps))),
            kv(
                th,
                "speed limits",
                format!("↑{} ↓{} kB/s", opt(n.limit_up_kbps), opt(n.limit_down_kbps)),
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
    let Some(inner) = framed(area, th, " Waiting to be mined · fees ", false, frame) else {
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
            lines.push(kv(
                th,
                "waiting",
                format!(
                    "{} transactions · {} · oldest {oldest}",
                    p.txs_total,
                    bytes(p.bytes_total)
                ),
            ));
            lines.push(kv(
                th,
                "their fees",
                format!(
                    "{} XMR · {} failing · {} double spends",
                    p.fee_total.to_xmr_string(),
                    p.num_failing,
                    p.num_double_spends
                ),
            ));
        }
        None => lines.push(Line::styled(
            m.node
                .err
                .clone()
                .unwrap_or_else(|| "checking what is waiting…".into()),
            th.dimmed(),
        )),
    }
    if let Some(f) = m.status.as_ref().map(|s| &s.node.fee) {
        // The four levels by the **names the Send screen offers**, so the two screens agree
        // (§2o): the old row labelled them by their priority index, which is a number nobody
        // chooses between.
        let fees = f.fees_pico.as_ref().map_or("—".to_string(), |v| {
            v.iter()
                .enumerate()
                .map(|(i, p)| format!("{} {}", FEE_LEVELS[i.min(3)], p.to_xmr_string()))
                .collect::<Vec<_>>()
                .join(" · ")
        });
        lines.push(kv(th, "fee levels", format!("{fees}  XMR per byte")));
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
        lines.push(Line::from_spans([
            Span::styled("• ", style),
            Span::styled(w.text.clone(), th.text()),
        ]));
        if let Some(action) = w.action.as_deref() {
            lines.push(Line::styled(format!("  {action}"), th.dimmed()));
        }
    }
    lines_at(lines, inner, true, frame);
}

// ---------------------------------------------------------------- Wallet

fn wallet_screen(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    if area.height < 4 {
        return;
    }
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(3), Constraint::Fill])
        .split(area);
    wallet_header(m, rows[0], frame);
    let open = m.status.as_ref().is_some_and(|s| s.wallet.open);
    if !open {
        wallet_empty(m, rows[1], frame);
        return;
    }
    // **Decision 7, said out loud** (v5.6): while a wallet is behind, the scanning is happening
    // inside the wallet service and closing this window does not stop it — exactly as
    // fbittorrent's transfers keep running when its window closes. The program has always
    // behaved this way and never said so.
    let sync = m.status.as_ref().and_then(wallet_sync_line);
    let rest = match &sync {
        Some(line) if rows[1].height > 3 => {
            let split = Flex::vertical()
                .constraints([Constraint::Fixed(2), Constraint::Fill])
                .split(rows[1]);
            lines_at(
                vec![
                    Line::styled(line.clone(), m.theme.warn()),
                    Line::styled(
                        "Syncing in the background. You can close this window.".to_string(),
                        m.theme.dimmed(),
                    ),
                ],
                split[0],
                false,
                frame,
            );
            split[1]
        }
        _ => rows[1],
    };
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
    let n = m.wallets.len();
    let dir = m.ctx.settings.wallet.dir.display().to_string();
    let open_label = format!("Open wallet…      choose one of the {n} in {dir}");
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
            (
                Button::CreateWallet,
                "c",
                "Create wallet…    a new one, kept on this machine",
            ),
            (
                Button::WalletDaemon,
                "W",
                "Start the wallet service   (not needed: opening or creating a wallet starts it)",
            ),
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

/// The four strings §7a.5 allows, and no fifth.
fn wallet_daemon_phrase(m: &Tui) -> String {
    if m.action
        .as_ref()
        .is_some_and(|a| a.label.contains("wallet service"))
    {
        return "starting…".into();
    }
    let Some(s) = m.status.as_ref() else {
        return "checking…".into();
    };
    if s.wallet.unit_active {
        return "running".into();
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

fn wallet_header(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Balance ", false, frame) else {
        return;
    };
    let Some(w) = m.status.as_ref().map(|s| &s.wallet) else {
        return;
    };
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
            format!("   {behind} blocks to go"),
            th.dimmed(),
        ));
    }
    line_at(spans, inner, frame);
}

fn subaddress_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let wide = area.width >= 62;
    let th = &m.theme;
    let focused = m.wallet.pane == Pane::Subaddresses;
    let title = " Your addresses — Enter copies · a new · l rename ";
    let Some(set) = &m.wallet.addresses else {
        if let Some(inner) = framed(area, th, title, focused, frame) {
            let (text, style) = match &m.wallet.err {
                Some(e) => (format!("{e}\nr retries"), th.err()),
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
    let fixed_sum: u16 = if wide { 3 + 11 + 22 } else { 3 + 22 };
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
                cells.push(fit(&a.label, 10));
            }
            cells.push(addr);
            cells.push(bal);
            TableRow::new(cells)
        })
        .collect();
    let (header, fixed): (&[&str], &[u16]) = if wide {
        (&["#", "NAME", "ADDRESS (✓ used)", "BALANCE"], &[3, 11, 22])
    } else {
        (&["#", "ADDRESS (✓ used)", "BALANCE"], &[3, 22])
    };
    table(m, area, title, rows, header, fixed, frame);
}

struct HistoryItem<'a> {
    row: &'a Transfer,
    th: &'a Theme,
    wh: Option<u64>,
    now: Option<i64>,
    wide: bool,
}

impl RenderItem for HistoryItem<'_> {
    fn render(&self, area: Rect, frame: &mut Frame, selected: bool, _skip: u16) {
        let (th, t) = (self.th, self.row);
        // Never recomputed here: `history::confirmations` / `history::locked` are the one
        // implementation (§5b), and they take the WALLET's height.
        let conf = history::confirmations(t, self.wh).map_or("—".to_string(), |c| c.to_string());
        let locked = match history::locked(t, self.wh, self.now) {
            Some(true) => format!("{:<11}", "locked"),
            Some(false) => format!("{:<11}", ""),
            None => format!("{:<11}", "—"),
        };
        let kind = t.kind.as_str();
        let mut text = format!(
            "{:<12} {:<7} {:>14} {:>6} {}",
            short_time(t.timestamp),
            kind,
            t.amount.to_xmr_string(),
            conf,
            locked
        );
        if self.wide {
            text.push_str(&format!(
                "  {:>12}  {}",
                t.fee.to_xmr_string(),
                fit(&t.note, 18)
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
    let mut header = format!(
        "{:<12} {:<7} {:>14} {:>6} {}",
        "WHEN", "WHAT", "AMOUNT", "CONFIRMS", "LOCKED"
    );
    if wide {
        header.push_str(&format!("  {:>12}  {}", "FEE", "NOTE"));
    }
    para(header, th.dimmed(), false).render(rows[1], frame);
    if let Some(e) = &m.wallet.rows_err {
        para(format!("Could not read your payments: {e}"), th.err(), true).render(rows[2], frame);
        return;
    }
    let shown = m.visible_history();
    if shown.is_empty() {
        para(
            "Nothing yet. Press R to check, / to filter, t to change what shows.".to_string(),
            th.dimmed(),
            true,
        )
        .render(rows[2], frame);
        return;
    }
    let items: Vec<HistoryItem> = shown
        .iter()
        .map(|row| HistoryItem {
            row,
            th,
            wh: m.wallet.wh,
            now: m.wallet.now,
            wide,
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
    let h = 13 + t.destinations.len() as u16;
    let Some(inner) = overlay(m, frame, 76, h, " Payment — n edits the note · Esc closes ")
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
    let mut lines = vec![
        pair("id", t.txid.clone()),
        pair(
            "what",
            format!(
                "{} · block {} · {}",
                t.kind.as_str(),
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
        pair(
            "confirms",
            format!(
                "{} · locked {}",
                history::confirmations(t, m.wallet.wh).map_or("—".into(), |c| c.to_string()),
                history::locked(t, m.wallet.wh, m.wallet.now)
                    .map_or("—".into(), |v| v.to_string())
            ),
        ),
        pair(
            "address #",
            format!(
                "{} (account {})",
                t.subaddr_index.minor, t.subaddr_index.major
            ),
        ),
        pair("unlocks at", t.unlock_time.to_string()),
    ];
    // A real Monero term where it has a value; an always-present empty row taught it as noise.
    if !t.payment_id.is_empty() {
        lines.push(pair("payment id", t.payment_id.clone()));
    }
    lines.push(pair("double spend", t.double_spend_seen.to_string()));
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
        para(
            format!(
                "No log yet. Your node writes it once it runs.\n{}",
                m.logs.path.display()
            ),
            th.dimmed(),
            true,
        )
        .render(rows[1], frame);
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
    let h = rows.len() as u16 + 5 + u16::from(input.is_some());
    let Some(inner) = overlay(m, frame, 68, h, title) else {
        return;
    };
    let bg = th.selection;
    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i == sel {
                th.selected()
            } else {
                th.text().bg(bg)
            };
            Line::styled(format!(" {r} "), style)
        })
        .collect();
    lines.push(Line::styled(
        "Enter chooses · Esc cancels",
        th.dimmed().bg(bg),
    ));
    lines_at(lines, inner, false, frame);
    if let Some((input, label)) = input {
        let row = inner.bottom().saturating_sub(1);
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
fn chip_row(m: &Tui, area: Rect, items: &[(Button, &str, &str)], frame: &mut Frame) {
    let th = &m.theme;
    let mut x = area.x;
    for (button, key, label) in items {
        let text = format!("[{key}] {label}");
        let w = text.width() as u16 + 1;
        if x + w > area.right() {
            return;
        }
        let rect = Rect::new(x, area.y, w - 1, 1);
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
    let Some(inner) = overlay(m, frame, 66, 13, " Create a wallet ") else {
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
            Constraint::Fixed(2),
            Constraint::Fill,
        ])
        .split(inner);
    for (i, (label, input)) in [
        ("name      ", name),
        ("password  ", pass),
        ("repeat    ", again),
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
        "seed lang ",
        fmonero_core::settings::SEED_LANGUAGES[lang].1,
        field == 3,
        frame,
    );
    if let Some(why) = err {
        para(why.to_string(), th.err().bg(bg), true).render(rows[4], frame);
    }
    lines_at(
        vec![
            Line::styled(
                "↑↓ fields · Enter next, then create · Esc discards",
                th.dimmed().bg(bg),
            ),
            // The one documented `Tab` exception, documented IN the modal (§7a.1).
            Line::styled(
                "Tab is off here. Press Esc to discard first.",
                th.warn().bg(bg),
            ),
        ],
        rows[5],
        false,
        frame,
    );
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
    let Some(inner) = overlay(m, frame, 74, 13, " Use this node ") else {
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
        Line::styled("· the wallet service will restart", th.text().bg(bg)),
    ];
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

/// §7a.4's `a` modal: **one** field, validated by the same function the CLI validates with.
///
/// There is deliberately no trusted toggle (revision 5): nothing in the schema supported one,
/// and a path to request it from this screen would make the confirm modal's own sentence an
/// untruth. The footer states the consequence instead of naming a CLI flag (§2o.1).
fn add_node_modal(m: &Tui, input: &TextInput, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = overlay(m, frame, 70, 8, " Add a node ") else {
        return;
    };
    m.hits.modal(inner);
    let bg = th.selection;
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(1), Constraint::Fixed(1), Constraint::Fill])
        .split(inner);
    let label = "address  ";
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
    lines_at(
        vec![
            Line::styled(
                "host:port, and the port is required".to_string(),
                th.dimmed().bg(bg),
            ),
            Line::styled(
                "Nodes you add here are treated as public.".to_string(),
                th.dimmed().bg(bg),
            ),
            Line::styled(
                "Enter adds it · Esc cancels".to_string(),
                th.dimmed().bg(bg),
            ),
        ],
        rows[2],
        true,
        frame,
    );
    let _ = rows[1];
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
    let quiet = 2u16;
    let modules = code.width() as u16;
    let side = modules + quiet * 2;
    let qr_rows = side.div_ceil(2);
    // `overlay` spends a row and a column on its border and one more of each on its padding,
    // so the interior it hands back is (w - 4, h - 4): asking for exactly the code's own
    // height left the address nowhere to go and reported a 120x40 terminal as too small.
    let text_rows = wrapped(address, side as usize).len() as u16;
    let Some(inner) = overlay(
        m,
        frame,
        side + 4,
        qr_rows + text_rows + 4,
        " Receive — Esc closes ",
    ) else {
        return;
    };
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
        let lines = wrapped(address, rest.width as usize)
            .into_iter()
            .map(|l| Line::styled(l, th.text().bg(th.selection)))
            .collect();
        lines_at(lines, rest, false, frame);
    }
}

/// The plain confirm modal — node up/down and the log level. `y` activates it, as §7a asks.
pub(super) fn confirm_dialog(th: &Theme, title: &str, message: &str) -> Dialog {
    Dialog::confirm(title.to_string(), message.to_string())
        .button_style(th.text())
        .primary_button_style(th.accented())
        .focused_button_style(th.selected())
}

fn help(m: &Tui, frame: &mut Frame) {
    let th = &m.theme;
    // `overlay` spends a row on each border and a row on each padding edge, so the interior is
    // `h - 4`: asking for `KEYS.len() + 3` rendered one row short and silently dropped the LAST
    // row of the table at every terminal size — which is the Log screen's `/ f n N`, bindings
    // that are implemented and documented nowhere else a user can reach (§7a: the overlay IS the
    // keymap).
    let Some(inner) = overlay(
        m,
        frame,
        100,
        KEYS.len() as u16 + 4,
        " Keys — any key closes ",
    ) else {
        return;
    };
    // §7a: in remote mode the keys that need the local daemon are marked from this same table,
    // so help and behaviour cannot drift — `confirm` toasts, and `b` has no bans to show.
    let remote = m.remote_mode();
    let lines = KEYS
        .iter()
        .map(|(k, a)| {
            let disabled = remote && matches!(*k, "u  d" | "L" | "b");
            // The mouse row reads its own setting rather than a copy of it, so the overlay's text
            // cannot drift from the policy actually in force (§7a.2's reason for making it a
            // setting and not a flag).
            let action = if disabled {
                format!("{a}  (not while you use a public node)")
            } else if *k == "mouse" && !m.ctx.settings.tui.mouse {
                // The settings key stays: it is a name the user types, which is §2o rule 5's
                // own carve-out. What goes is the shell command (§2o.1).
                "off — turn on tui.mouse in your settings".to_string()
            } else {
                (*a).to_string()
            };
            Line::from_spans([
                Span::styled(format!("{k:<22}"), th.key().bg(th.selection)),
                Span::styled(
                    action,
                    if disabled {
                        th.dimmed().bg(th.selection)
                    } else {
                        th.text().bg(th.selection)
                    },
                ),
            ])
        })
        .collect();
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
fn kv(th: &Theme, label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from_spans([
        Span::styled(format!("{label:<11}"), th.dimmed()),
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

pub(super) fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn rate(bps: u64) -> String {
    format!("{}/s", bytes(bps))
}

pub(super) fn age(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}h {m:02}m ago")
    } else if m > 0 {
        format!("{m}m {s:02}s ago")
    } else {
        format!("{s}s ago")
    }
}

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
