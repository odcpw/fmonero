//! The mouse hit map (DESIGN.md §7a.2, v5.5).
//!
//! **Why this file exists rather than ftui's hit grid.** ftui 0.7 has `Frame::register_hit`
//! (`ftui-render-0.7.0/src/frame.rs:751-769`), `Frame::hit_test` (`:898-901`) and widget-side
//! `hit_id` builders — and under `App::fullscreen` they do **nothing**: the runtime renders
//! through `Frame::from_buffer` (`ftui-runtime-0.7.0/src/program.rs:7192`) and measures through
//! `Frame::new` (`:7418`), never `Frame::with_hit_grid` (`frame.rs:567-580`), so `hit_grid` is
//! `None`, `register_hit` returns `false` having registered nothing, and the frame is consumed
//! after `view()` (`program.rs:7215`) before `update()` could query it.
//! `ProgramSimulator::capture_frame` is the same (`simulator.rs:348`). Code written against
//! that API compiles, runs and silently does nothing — found by a user clicking, not by a test.
//!
//! So fmonero hit-tests the way every interactive ftui screen does: **`view()` records the
//! rects it laid out and `update()` resolves a click against them.** `Model::view` takes
//! `&self` (`program.rs:378`), so the cache needs interior mutability — `Cell<Rect>` where one
//! rect is enough (`Rect` is `Copy`, and a `RefCell` there would buy a runtime borrow check for
//! nothing) and `RefCell<Vec<_>>` where a list of them is.
//!
//! One `match` owns the priority chain, so a reviewer can read it end to end, and the same
//! `Map` can be built in a test with no terminal.

use std::cell::{Cell, RefCell};

use ftui::layout::Rect;

/// A scrollable list or table, named so a wheel can be routed to the one **under the cursor**
/// rather than the focused one (§7a.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HitPane {
    /// The Node screen's peers/bans table.
    NodeList,
    /// The Node screen's public-node picker (§7a.4).
    Picker,
    /// The Wallet screen's subaddress table.
    Subaddresses,
    /// The Wallet screen's history list.
    History,
    /// The Log viewer.
    Log,
    /// A modal's own list (the open-wallet picker).
    ModalList,
}

/// Every rendered `[key]` chip. **It names the ACTION, not the key**, so re-binding a key
/// cannot silently re-point a button (§7a.2's "two affordances, one label").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Button {
    OpenWallet,
    CreateWallet,
    CloseWallet,
    WalletDaemon,
    RefreshWallet,
    Qr,
    NodeUp,
    NodeDown,
    Probe,
    RefreshList,
    AddNode,
    UseNode,
    PrevNode,
    /// v5.6, §7a.4's `l`: go back to your own node.
    UseLocal,
    /// Round 4's M16: the peers/blocked-addresses swap, which was a `b:` prefix inside a pane
    /// title — a style used nowhere else in the product and the one place a reader met a key
    /// without a chip around it.
    SwapPeers,
    Help,
    NextScreen,
}

/// What is under a coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// The tab strip: a screen index.
    Tab(usize),
    /// A list or table row in a named pane.
    Row { pane: HitPane, row: usize },
    /// A Send form input, by its index in the form's own order.
    Field(usize),
    /// A priority chip. Clicking one **selects that priority** rather than cycling, which is
    /// also more discoverable than four presses of `p` (§7a.2, revision 6).
    Priority(u32),
    /// One arrow of a `◂ value ▸` selector: `-1` or `+1` (§7a.2, revision 6). Every control the
    /// keymap reaches with `←→` renders its two arrows as hit targets.
    Arrow { which: Selector, delta: i32 },
    /// A rendered `[key]` chip.
    Button(Button),
    /// The focused modal's nth button.
    ModalButton(usize),
    /// The wheel, resolved to the pane under the cursor.
    Scroll { pane: HitPane, up: bool },
    /// Outside an open modal.
    Backdrop,
}

/// Which `◂ value ▸` selector an [`Target::Arrow`] belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Selector {
    /// The Send screen's account row.
    Account,
    /// The create-wallet modal's seed language.
    Language,
}

/// A pane's geometry: where its rows start and how tall a row is, so one place computes a row
/// index and `hit.rs` is that place.
#[derive(Clone, Copy, Debug)]
struct PaneGeom {
    pane: HitPane,
    /// The whole pane, including its border — what a wheel is routed by.
    area: Rect,
    /// The first row's y, inside the border and below any header.
    first_row_y: u16,
    /// How many rows are drawn.
    rows: u16,
    /// The index of the first drawn row, so a scrolled table maps a click to the right item.
    offset: usize,
}

/// The rects `view()` laid out, as `update()` needs them.
#[derive(Default)]
pub(super) struct Map {
    tabs: RefCell<Vec<(usize, Rect)>>,
    panes: RefCell<Vec<PaneGeom>>,
    buttons: RefCell<Vec<(Button, Rect)>>,
    fields: RefCell<Vec<(usize, Rect)>>,
    priorities: RefCell<Vec<(u32, Rect)>>,
    arrows: RefCell<Vec<(Selector, i32, Rect)>>,
    /// The focused modal's own box. A click **outside** it is [`Target::Backdrop`].
    modal: Cell<Option<Rect>>,
    modal_buttons: RefCell<Vec<Rect>>,
    /// The `?` overlay is up: any click anywhere closes it, so it short-circuits everything.
    overlay: Cell<bool>,
    /// The status line's two labelled segments.
    help: Cell<Option<Rect>>,
    next_screen: Cell<Option<Rect>>,
}

impl Map {
    /// Called first in `view()`. Without it a screen that stopped drawing a button would leave
    /// the button clickable — a hit map is only true for the frame that built it.
    pub fn begin(&self) {
        self.tabs.borrow_mut().clear();
        self.panes.borrow_mut().clear();
        self.buttons.borrow_mut().clear();
        self.fields.borrow_mut().clear();
        self.priorities.borrow_mut().clear();
        self.arrows.borrow_mut().clear();
        self.modal_buttons.borrow_mut().clear();
        self.modal.set(None);
        self.overlay.set(false);
        self.help.set(None);
        self.next_screen.set(None);
    }

    pub fn tab(&self, index: usize, area: Rect) {
        self.tabs.borrow_mut().push((index, area));
    }
    pub fn pane(&self, pane: HitPane, area: Rect, first_row_y: u16, rows: u16, offset: usize) {
        self.panes.borrow_mut().push(PaneGeom {
            pane,
            area,
            first_row_y,
            rows,
            offset,
        });
    }
    pub fn button(&self, button: Button, area: Rect) {
        self.buttons.borrow_mut().push((button, area));
    }
    pub fn field(&self, index: usize, area: Rect) {
        self.fields.borrow_mut().push((index, area));
    }
    pub fn priority(&self, level: u32, area: Rect) {
        self.priorities.borrow_mut().push((level, area));
    }
    pub fn arrow(&self, which: Selector, delta: i32, area: Rect) {
        self.arrows.borrow_mut().push((which, delta, area));
    }
    pub fn modal(&self, area: Rect) {
        self.modal.set(Some(area));
    }
    pub fn modal_button(&self, area: Rect) {
        self.modal_buttons.borrow_mut().push(area);
    }
    pub fn overlay(&self) {
        self.overlay.set(true);
    }
    pub fn status(&self, help: Rect, next_screen: Rect) {
        self.help.set(Some(help));
        self.next_screen.set(Some(next_screen));
    }

    /// **The priority chain, first match wins** (§7a.2), and it is the app-shell order the
    /// FrankenTUI skill's §8 describes minus the command palette fmonero does not have:
    /// overlay → modal → tab strip → pane content → status line → nothing.
    ///
    /// `scroll` is `Some(up)` for a wheel event, which resolves only against panes: a wheel over
    /// the tab strip does nothing, because otherwise it would change screens while someone is
    /// scrolling a list near the top edge.
    pub fn at(&self, x: u16, y: u16, scroll: Option<bool>) -> Option<Target> {
        // 1. the `?` overlay — a click anywhere closes it. An overlay that covers the screen and
        //    ignores clicks is a screen the user cannot leave with the device that opened it.
        if self.overlay.get() {
            return Some(Target::Button(Button::Help));
        }
        // 2. an open modal — its buttons, then its own list; outside its box is the backdrop.
        if let Some(box_) = self.modal.get() {
            if let Some(up) = scroll {
                return self
                    .pane_at(x, y)
                    .filter(|g| g.pane == HitPane::ModalList)
                    .map(|g| Target::Scroll { pane: g.pane, up });
            }
            for (i, r) in self.modal_buttons.borrow().iter().enumerate() {
                if contains(*r, x, y) {
                    return Some(Target::ModalButton(i));
                }
            }
            if let Some(g) = self
                .pane_at(x, y)
                .filter(|g| g.pane == HitPane::ModalList)
                .and_then(|g| self.row_in(&g, y))
            {
                return Some(g);
            }
            for (i, r) in self.fields.borrow().iter() {
                if contains(*r, x, y) {
                    return Some(Target::Field(*i));
                }
            }
            for (which, delta, r) in self.arrows.borrow().iter() {
                if contains(*r, x, y) {
                    return Some(Target::Arrow {
                        which: *which,
                        delta: *delta,
                    });
                }
            }
            return (!contains(box_, x, y)).then_some(Target::Backdrop);
        }
        // A wheel resolves against panes only, and against the pane under the CURSOR.
        if let Some(up) = scroll {
            return self
                .pane_at(x, y)
                .map(|g| Target::Scroll { pane: g.pane, up });
        }
        // 3. the tab strip — the fix for "the tabs look clickable and are not".
        for (i, r) in self.tabs.borrow().iter() {
            if contains(*r, x, y) {
                return Some(Target::Tab(*i));
            }
        }
        // 4. pane content: buttons and fields first (they sit inside a pane's area), then rows.
        for (b, r) in self.buttons.borrow().iter() {
            if contains(*r, x, y) {
                return Some(Target::Button(*b));
            }
        }
        for (i, r) in self.fields.borrow().iter() {
            if contains(*r, x, y) {
                return Some(Target::Field(*i));
            }
        }
        for (level, r) in self.priorities.borrow().iter() {
            if contains(*r, x, y) {
                return Some(Target::Priority(*level));
            }
        }
        for (which, delta, r) in self.arrows.borrow().iter() {
            if contains(*r, x, y) {
                return Some(Target::Arrow {
                    which: *which,
                    delta: *delta,
                });
            }
        }
        if let Some(t) = self.pane_at(x, y).and_then(|g| self.row_in(&g, y)) {
            return Some(t);
        }
        // 5. the status line's two segments, the only two already labelled like controls.
        if self.help.get().is_some_and(|r| contains(r, x, y)) {
            return Some(Target::Button(Button::Help));
        }
        if self.next_screen.get().is_some_and(|r| contains(r, x, y)) {
            return Some(Target::Button(Button::NextScreen));
        }
        // 6. nothing. A click that hits no target is discarded silently — no beep, no toast, no
        //    focus change: a click that does something arbitrary is worse than one that does
        //    nothing.
        None
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<PaneGeom> {
        self.panes
            .borrow()
            .iter()
            .find(|g| contains(g.area, x, y))
            .copied()
    }

    fn row_in(&self, g: &PaneGeom, y: u16) -> Option<Target> {
        if g.rows == 0 || y < g.first_row_y {
            return None;
        }
        let row = (y - g.first_row_y) as usize;
        (row < g.rows as usize).then_some(Target::Row {
            pane: g.pane,
            row: g.offset + row,
        })
    }
}

/// `Rect::contains` (`ftui-core-0.7.0/src/geometry.rs:170-172`) is the whole of the hit test,
/// given a cached rect — wrapped only so a zero-area rect can never swallow a click.
fn contains(r: Rect, x: u16, y: u16) -> bool {
    r.width > 0 && r.height > 0 && r.contains(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn the_overlay_swallows_every_click_before_anything_else() {
        let m = Map::default();
        m.begin();
        m.tab(0, r(0, 0, 10, 1));
        m.overlay();
        // With the overlay up, a click on a tab closes the overlay and does NOT switch screens.
        assert_eq!(m.at(3, 0, None), Some(Target::Button(Button::Help)));
    }

    #[test]
    fn a_modal_backdrop_click_is_esc_and_a_modal_button_wins_over_it() {
        let m = Map::default();
        m.begin();
        m.tab(1, r(10, 0, 10, 1));
        m.modal(r(20, 5, 40, 10));
        m.modal_button(r(22, 13, 8, 1));
        assert_eq!(m.at(24, 13, None), Some(Target::ModalButton(0)));
        // Inside the modal but on nothing: no target, NOT the backdrop.
        assert_eq!(m.at(30, 7, None), None);
        // Outside it: the backdrop, and the tab strip underneath is unreachable.
        assert_eq!(m.at(12, 0, None), Some(Target::Backdrop));
    }

    #[test]
    fn rows_account_for_the_panes_scroll_offset() {
        let m = Map::default();
        m.begin();
        m.pane(HitPane::Picker, r(0, 2, 80, 12), 4, 8, 17);
        // The third drawn row of a table scrolled to 17 is item 19.
        assert_eq!(
            m.at(10, 6, None),
            Some(Target::Row {
                pane: HitPane::Picker,
                row: 19
            })
        );
        // Above the first row (the header) is not a row.
        assert_eq!(m.at(10, 3, None), None);
        // Past the last drawn row is not a row either.
        assert_eq!(m.at(10, 13, None), None);
    }

    #[test]
    fn the_wheel_goes_to_the_pane_under_the_cursor_and_never_to_the_tab_strip() {
        let m = Map::default();
        m.begin();
        m.tab(0, r(0, 0, 10, 1));
        m.pane(HitPane::History, r(40, 2, 40, 10), 3, 8, 0);
        m.pane(HitPane::Subaddresses, r(0, 2, 39, 10), 3, 8, 0);
        assert_eq!(
            m.at(50, 5, Some(true)),
            Some(Target::Scroll {
                pane: HitPane::History,
                up: true
            })
        );
        assert_eq!(
            m.at(5, 5, Some(false)),
            Some(Target::Scroll {
                pane: HitPane::Subaddresses,
                up: false
            })
        );
        // Over the tab strip a wheel does nothing: it would change screens while someone is
        // scrolling a list near the top edge.
        assert_eq!(m.at(3, 0, Some(true)), None);
    }

    #[test]
    fn every_arrow_controlled_selector_is_a_target() {
        let m = Map::default();
        m.begin();
        m.priority(3, r(20, 8, 6, 1));
        m.arrow(Selector::Account, -1, r(10, 9, 1, 1));
        m.arrow(Selector::Account, 1, r(16, 9, 1, 1));
        assert_eq!(m.at(22, 8, None), Some(Target::Priority(3)));
        assert_eq!(
            m.at(10, 9, None),
            Some(Target::Arrow {
                which: Selector::Account,
                delta: -1
            })
        );
        assert_eq!(
            m.at(16, 9, None),
            Some(Target::Arrow {
                which: Selector::Account,
                delta: 1
            })
        );
    }

    #[test]
    fn begin_forgets_the_previous_frame() {
        let m = Map::default();
        m.begin();
        m.button(Button::Probe, r(2, 2, 10, 1));
        assert_eq!(m.at(4, 2, None), Some(Target::Button(Button::Probe)));
        // A screen that stops drawing a chip must stop accepting clicks on it.
        m.begin();
        assert_eq!(m.at(4, 2, None), None);
    }

    #[test]
    fn a_zero_area_rect_never_swallows_a_click() {
        let m = Map::default();
        m.begin();
        m.button(Button::Qr, r(5, 5, 0, 1));
        m.button(Button::Probe, r(5, 5, 4, 1));
        assert_eq!(m.at(5, 5, None), Some(Target::Button(Button::Probe)));
    }
}
