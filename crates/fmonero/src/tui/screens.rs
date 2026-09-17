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

use fmonero_core::{history, units::Layout, Severity, Transfer};

use super::theme::Theme;
use super::{Modal, NodeView, Pane, Screen, Tui, KEYS, SPINNER};

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
    if b.width == 0 || b.height == 0 {
        return;
    }
    let th = &m.theme;
    let tier = tier_of(b);
    if tier == Tier::Tiny {
        para("fmonero — terminal too small (need 40x10)", th.warn(), true)
            .render(Rect::new(b.x, b.y, b.width, b.height.min(3)), frame);
        return;
    }
    let banner = u16::from(m.status.as_ref().is_some_and(|s| s.pending.is_some()));
    let rows = Flex::vertical()
        .constraints([
            Constraint::Fixed(1),
            Constraint::Fixed(banner),
            Constraint::Fill,
            Constraint::Fixed(1),
        ])
        .split(b);
    tab_strip(m, rows[0], frame, tier);
    if banner == 1 {
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
                vec![
                    format!("no wallets in {}", m.ctx.settings.wallet.dir.display()),
                    "fmonero wallet create <name> --yes".into(),
                ]
            } else {
                names.clone()
            };
            list_modal(
                m,
                " Open a wallet — Enter · Esc cancels ",
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
        None => {}
    }
    if let super::send::Stage::Built(plan) = &m.send.stage {
        super::send::view_confirm(m, plan, frame);
    }
    if m.wallet.detail {
        detail_modal(m, frame);
    }
    if m.show_help {
        help(m, frame);
    }
}

// ---------------------------------------------------------------- chrome

fn tab_strip(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    let th = &m.theme;
    if tier == Tier::Narrow {
        let mut spans = Vec::new();
        for (i, label) in ["1", "2", "3", "4"].iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("·", th.dimmed()));
            }
            let style = if i == m.screen.index() {
                th.accented()
            } else {
                th.dimmed()
            };
            spans.push(Span::styled((*label).to_string(), style));
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

/// The §7a red banner: on **every** screen while a `pending.json` record is unresolved.
fn pending_banner(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(p) = m.status.as_ref().and_then(|s| s.pending.as_ref()) else {
        return;
    };
    let txid: String = p
        .txs
        .first()
        .map_or("—".into(), |t| t.txid.chars().take(16).collect());
    para(
        format!(
            "SEND IN FLIGHT {} XMR → {} · {} of {} relayed · {txid} · fmonero wallet history --limit 5 · fmonero wallet clear-pending --yes",
            p.total_amount_xmr, short_addr(&p.dest), p.relayed_count, p.total_count
        ),
        th.err(),
        false,
    )
    .render(area, frame);
}

fn status_line(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    StatusLine::new()
        .style(th.dimmed())
        .right(StatusItem::key_hint("?", "help"))
        .render(area, frame);
    let (node, wallet) = (m.node_badge(), m.wallet_badge());
    let mut spans = vec![
        Span::styled("●", th.badge(node)),
        Span::styled(format!(" node {node} "), th.text()),
    ];
    if let Some(s) = &m.status {
        if let Some(h) = s.node.height {
            spans.push(Span::styled(format!("{h} "), th.dimmed()));
        }
        if let Some(out) = s.node.outgoing_connections_count {
            let inc = s.node.incoming_connections_count.unwrap_or(0);
            spans.push(Span::styled(format!("{out}↑{inc}↓ "), th.dimmed()));
        }
        spans.push(Span::styled("●", th.badge(wallet)));
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
        Rect::new(area.x, area.y, area.width.saturating_sub(8), 1),
        frame,
    );
}

// ---------------------------------------------------------------- Node

fn node_screen(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    if area.height < 4 {
        return;
    }
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
            let right = Flex::vertical()
                .constraints([Constraint::Fixed(5), Constraint::Fill])
                .split(top[1]);
            net_pane(m, right[0], frame, true);
            pool_pane(m, right[1], frame);
            let bottom = Flex::horizontal()
                .constraints([Constraint::Percentage(62.0), Constraint::Fill])
                .split(rows[1]);
            list_pane(m, bottom[0], frame, bottom[0].width >= 70);
            warnings_pane(m, bottom[1], frame);
        }
        Tier::Medium => {
            let rows = Flex::vertical()
                .constraints([
                    Constraint::Fixed(10),
                    Constraint::Fixed(6),
                    Constraint::Fill,
                    Constraint::Fixed(5),
                ])
                .split(area);
            node_hero(m, rows[0], frame);
            pool_pane(m, rows[1], frame);
            list_pane(m, rows[2], frame, false);
            warnings_pane(m, rows[3], frame);
        }
        _ => {
            let rows = Flex::vertical()
                .constraints([Constraint::Fixed(9), Constraint::Fill])
                .split(area);
            node_hero(m, rows[0], frame);
            warnings_pane(m, rows[1], frame);
        }
    }
}

fn node_hero(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Node ", m.status_err.is_none(), frame) else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    let mut gauge = None;
    if let Some(e) = &m.status_err {
        lines.push(Line::styled(format!("status failed: {e}"), th.err()));
        lines.push(Line::styled("press r to try again", th.dimmed()));
    } else if let Some(s) = &m.status {
        let n = &s.node;
        lines.push(Line::from_spans([
            Span::styled("● ", th.badge(m.node_badge())),
            Span::styled(m.node_phrase(), th.title()),
        ]));
        if !n.reachable {
            lines.push(Line::styled(
                if m.monero_missing() {
                    "Monero is not installed. omarchy pkg add monero, then fmonero setup --units"
                } else {
                    "Node is not running — press u to start it"
                },
                th.warn(),
            ));
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
                "sync",
                format!("{:13}{pct:.1}%  {} / {target}", "", opt(n.height)),
            ));
            if let Some(eta) = m.sync_eta() {
                lines.push(kv(th, "eta", eta));
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
        lines.push(kv(
            th,
            "version",
            format!(
                "{} · fork v{} · {}",
                n.version.clone().unwrap_or_else(|| "—".into()),
                opt(n.hard_fork_version),
                n.nettype.clone().unwrap_or_else(|| "—".into())
            ),
        ));
        if n.layout == Layout::SystemUnit {
            lines.push(Line::styled(
                "Node owned by the system unit — read-only (u/d/L disabled)",
                th.warn(),
            ));
        }
        if n.restricted == Some(true) {
            lines.push(Line::styled(
                "restricted RPC: peers, net stats and bans have no source (§1b.2)",
                th.warn(),
            ));
        }
    } else {
        lines.push(Line::styled("reading status…", th.dimmed()));
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
    if m.node_restricted() {
        let method = if bans { "get_bans" } else { "get_connections" };
        empty(
            &format!("not available on a restricted node ({method})"),
            frame,
        );
        return;
    }
    let Some(extras) = extras else {
        let text = m
            .node
            .err
            .clone()
            .unwrap_or_else(|| "reading the node…".into());
        empty(&text, frame);
        return;
    };
    if bans {
        if extras.bans.is_empty() {
            empty("no bans · fmonero node ban <host>", frame);
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
        empty("no connections yet", frame);
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
            &["HOST", "STATE", "HEIGHT", "PRUNE", "AVG ↓/↑"],
            &[23, 14, 9, 6],
        )
    } else {
        (&["HOST", "STATE", "HEIGHT"], &[23, 14])
    };
    table(m, area, &title, rows, header, widths, frame);
}

fn net_pane(m: &Tui, area: Rect, frame: &mut Frame, sparkline: bool) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Net ", false, frame) else {
        return;
    };
    let Some(n) = m.status.as_ref().map(|s| &s.node.net) else {
        para("node unreachable", th.dimmed(), false).render(inner, frame);
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
                "limits",
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
            para(
                "the rx sparkline fills as samples arrive",
                th.dimmed(),
                true,
            )
            .render(rows[1], frame);
        }
    }
}

fn pool_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Mempool · fees ", false, frame) else {
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
                "pool",
                format!(
                    "{} txs · {} · oldest {oldest}",
                    p.txs_total,
                    bytes(p.bytes_total)
                ),
            ));
            lines.push(kv(
                th,
                "pool fees",
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
                .unwrap_or_else(|| "reading the transaction pool…".into()),
            th.dimmed(),
        )),
    }
    if let Some(f) = m.status.as_ref().map(|s| &s.node.fee) {
        let fees = f.fees_pico.as_ref().map_or("—".to_string(), |v| {
            v.iter()
                .enumerate()
                .map(|(i, p)| format!("{}:{}", i + 1, p.to_xmr_string()))
                .collect::<Vec<_>>()
                .join("  ")
        });
        lines.push(kv(th, "fee/byte", fees));
    }
    lines_at(lines, inner, true, frame);
}

fn warnings_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let th = &m.theme;
    let Some(inner) = framed(area, th, " Warnings ", false, frame) else {
        return;
    };
    let empty = Vec::new();
    let list = m.status.as_ref().map_or(&empty, |s| &s.warnings);
    if list.is_empty() {
        para("none", th.dimmed(), false).render(inner, frame);
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
        lines.push(Line::from_spans([
            Span::styled(format!("{} ", w.id), style),
            Span::styled(w.text.clone(), th.text()),
        ]));
        if !w.hint.is_empty() {
            lines.push(Line::styled(format!("  hint: {}", w.hint), th.dimmed()));
        }
    }
    lines_at(lines, inner, true, frame);
}

// ---------------------------------------------------------------- Wallet

fn wallet_screen(m: &Tui, area: Rect, frame: &mut Frame, tier: Tier) {
    if area.height < 4 {
        return;
    }
    let th = &m.theme;
    let rows = Flex::vertical()
        .constraints([Constraint::Fixed(3), Constraint::Fill])
        .split(area);
    wallet_header(m, rows[0], frame);
    let open = m.status.as_ref().is_some_and(|s| s.wallet.open);
    if !open {
        let Some(inner) = framed(rows[1], th, " Wallet ", true, frame) else {
            return;
        };
        let text = match m.status.as_ref() {
            Some(s) if !s.wallet.reachable => {
                "Wallet daemon is not running (press o to start it and open a wallet)"
            }
            Some(_) => "No wallet open — press o",
            None => "reading status…",
        };
        para(text.to_string(), th.dimmed(), true).render(inner, frame);
        return;
    }
    match tier {
        Tier::Wide => {
            let cols = Flex::horizontal()
                .constraints([Constraint::Percentage(42.0), Constraint::Fill])
                .split(rows[1]);
            subaddress_pane(m, cols[0], frame);
            history_pane(m, cols[1], frame, true);
        }
        Tier::Medium => {
            let split = Flex::vertical()
                .constraints([Constraint::Fixed(8), Constraint::Fill])
                .split(rows[1]);
            subaddress_pane(m, split[0], frame);
            history_pane(m, split[1], frame, false);
        }
        _ => match m.wallet.pane {
            Pane::Subaddresses => subaddress_pane(m, rows[1], frame),
            Pane::History => history_pane(m, rows[1], frame, false),
        },
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
        spans.push(Span::styled(
            format!("   ↑ locked for {blocks} blocks (≈ {} min)", blocks * 2),
            th.warn(),
        ));
    }
    if let Some(name) = &w.name {
        spans.push(Span::styled(format!("   {name}"), th.dimmed()));
    }
    if let Some(behind) = w.blocks_behind.filter(|b| *b > 0) {
        spans.push(Span::styled(
            format!("   {behind} blocks behind"),
            th.dimmed(),
        ));
    }
    line_at(spans, inner, frame);
}

fn subaddress_pane(m: &Tui, area: Rect, frame: &mut Frame) {
    let wide = area.width >= 62;
    let th = &m.theme;
    let focused = m.wallet.pane == Pane::Subaddresses;
    let title = " Subaddresses — Enter copies · a new · l label ";
    let Some(set) = &m.wallet.addresses else {
        if let Some(inner) = framed(area, th, title, focused, frame) {
            let text = m.wallet.err.clone().unwrap_or_else(|| "loading…".into());
            para(text, th.dimmed(), true).render(inner, frame);
        }
        return;
    };
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
                .map_or("—".to_string(), |s| s.balance.to_xmr_string());
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
        (&["#", "LABEL", "ADDRESS ✓=used", "BALANCE"], &[3, 11, 22])
    } else {
        (&["#", "ADDRESS ✓=used", "BALANCE"], &[3, 22])
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
            Some(true) => "locked",
            Some(false) => "",
            None => "—",
        };
        let kind = t.kind.as_str();
        let mut text = format!(
            "{} {:<7} {:>14} {:>6} {}",
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
        " History  {} rows · t: {} ",
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
    search_row(&m.wallet.filter, m.wallet.filter_open, th, rows[0], frame);
    let header = if wide {
        "WHEN         TYPE            AMOUNT   CONF               FEE  NOTE"
    } else {
        "WHEN         TYPE            AMOUNT   CONF"
    };
    para(header.to_string(), th.dimmed(), false).render(rows[1], frame);
    if let Some(e) = &m.wallet.rows_err {
        para(format!("history is unavailable: {e}"), th.err(), true).render(rows[2], frame);
        return;
    }
    let shown = m.visible_history();
    if shown.is_empty() {
        para(
            "No rows. R refreshes the wallet, / filters, t cycles the type.".to_string(),
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
    StatefulWidget::render(&list, rows[2], frame, &mut m.wallet.list.borrow_mut());
    if m.wallet.wh.is_none() && rows[2].height > 0 {
        // §4.1: `—` rather than 0, and a footer saying which guarantee was given up.
        para(
            "cached: confirmations unknown (no daemon call)".to_string(),
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
    let Some(inner) = overlay(
        m,
        frame,
        76,
        h,
        " Transfer — n edits the note · Esc closes ",
    ) else {
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
        pair("txid", t.txid.clone()),
        pair(
            "type",
            format!(
                "{} · height {} · {}",
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
                "{} · locked (est.) {}",
                history::confirmations(t, m.wallet.wh).map_or("—".into(), |c| c.to_string()),
                history::locked(t, m.wallet.wh, m.wallet.now)
                    .map_or("—".into(), |v| v.to_string())
            ),
        ),
        pair(
            "subaddr",
            format!("{}/{}", t.subaddr_index.major, t.subaddr_index.minor),
        ),
        pair("unlock", t.unlock_time.to_string()),
        pair("payment id", dash(&t.payment_id)),
        pair("double sp", t.double_spend_seen.to_string()),
        pair("note", dash(&t.note)),
        Line::styled("destinations", dim),
    ];
    if t.destinations.is_empty() {
        lines.push(Line::styled("  — (an incoming transfer has none)", dim));
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
    let title = format!(
        " Log  {}  follow {} ",
        m.logs.path.display(),
        if m.logs.follow { "on" } else { "off" }
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
    search_row(&m.logs.search, m.logs.search_open, th, rows[0], frame);
    if m.logs.empty {
        para(
            format!(
                "no log yet at {} — the node writes it once it runs",
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
    let h = rows.len() as u16 + 4 + u16::from(input.is_some());
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
    lines.push(Line::styled("Enter · Esc cancels", th.dimmed().bg(bg)));
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
    let text_rows = wrapped(address, 72).len() as u16;
    let Some(inner) = overlay(
        m,
        frame,
        side + 4,
        qr_rows + text_rows + 2,
        " Receive — Esc closes ",
    ) else {
        return;
    };
    let fits = inner.height > qr_rows && inner.width >= side;
    if fits {
        let colors = code.to_colors();
        let mut painter = Painter::new(side, side, Mode::HalfBlock);
        // Lit sub-pixels are the LIGHT modules (and the quiet zone), so the code reads
        // dark-on-light whatever the terminal's palette is (`Theme::qr`).
        for y in 0..side {
            for x in 0..side {
                let inside = x >= quiet && y >= quiet && x < quiet + modules && y < quiet + modules;
                let dark = inside
                    && colors[(y - quiet) as usize * modules as usize + (x - quiet) as usize]
                        == QrColor::Dark;
                if !dark {
                    painter.point(i32::from(x), i32::from(y));
                }
            }
        }
        Canvas::from_painter(&painter)
            .style(th.qr())
            .render(Rect::new(inner.x, inner.y, side, qr_rows), frame);
    } else {
        para(
            format!("terminal too small for a QR (needs {side}x{qr_rows}) — the address is below"),
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
    let Some(inner) = overlay(
        m,
        frame,
        100,
        KEYS.len() as u16 + 3,
        " Keys — any key closes ",
    ) else {
        return;
    };
    let lines = KEYS
        .iter()
        .map(|(k, a)| {
            Line::from_spans([
                Span::styled(format!("{k:<22}"), th.key().bg(th.selection)),
                Span::styled((*a).to_string(), th.text().bg(th.selection)),
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
    let state = if m.screen == Screen::Wallet {
        &m.wallet.table
    } else {
        &m.node.table
    };
    StatefulWidget::render(&t, area, frame, &mut state.borrow_mut());
}

fn search_row(input: &TextInput, open: bool, th: &Theme, area: Rect, frame: &mut Frame) {
    let style = if open { th.accented() } else { th.dimmed() };
    para("/ ".to_string(), style, false).render(area, frame);
    input.render(
        Rect::new(area.x + 2, area.y, area.width.saturating_sub(2).max(1), 1),
        frame,
    );
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
