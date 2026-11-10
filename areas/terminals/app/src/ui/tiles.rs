//! The Terminals page: the Sessions rail, the strip above the windows, and
//! the windows themselves, each a session's terminal, a state, or the
//! launcher. The terminal takes every key and the mouse.

use super::page::{Act, Geo, MenuItem, Terminals};
use super::place;
use crate::remote::SharedScreen;
use crate::remotes::PaneAt;
use crate::store::sessions::{self, Key};
use crate::store::workspace::{Binding, MAX_WINDOWS, TileState};
use crate::term::links::{self, Link};
use crate::term::paint::{self, Look};
use crate::term::{keys, mouse};
use crate::theme::{Theme, alpha, size, theme};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use gpui::{
    AnyElement, Bounds, ClipboardItem, ContentMask, Context, DispatchPhase, ElementInputHandler,
    EntityInputHandler, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, ScrollDelta, ScrollEvent, UTF16Selection, Window, canvas, deferred, div, point,
    prelude::*, px, relative,
};
use parvi_app_kit::Ask;
use parvi_app_kit::host::Request;
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_core::NodeId;
use std::ops::Range;

mod drag;

/// What Claim does, said where it is offered (CHROTE's words).
pub const CLAIM_EXPLANATION: &str =
    "Set the tmux window to this device's size. Other devices keep watching, at that size.";

/// Why a pane cannot be claimed: tmux keeps its window at a fixed size.
fn pinned(s: &parvi_terminals_contract::Session, pane: u32) -> Option<String> {
    if s.pinned != Some(true) {
        return None;
    }
    let p = s.panes.iter().find(|p| p.id == pane)?;
    Some(format!(
        "Pinned at {}x{}. tmux window-size is manual on this window, so Parvi cannot resize it.",
        p.cols, p.rows
    ))
}

/// What a window says while a drop on it would land there.
fn drop_note(t: &Theme) -> gpui::Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(t.accent, 0.12))
        .child(
            kit::panel(t)
                .px(px(14.))
                .py(px(8.))
                .text_color(t.text)
                .child("Release to add"),
        )
}

/// A session dragged from the rail, or a tag from window `from`, onto a
/// window or a seam.
#[derive(Clone)]
pub struct Dragged {
    pub key: Key,
    pub from: Option<usize>,
}

impl Dragged {
    /// Whether dropping on window `i` would change anything: a tag dropped
    /// on the window it lives in has nowhere to land, so it shows no
    /// "Release to add".
    pub fn lands_on(&self, i: usize) -> bool {
        self.from != Some(i)
    }
}

pub struct DragPreview {
    pub name: String,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        kit::panel(t)
            .px(px(10.))
            .py(px(4.))
            .text_color(t.text)
            .child(self.name.clone())
    }
}

/// What a window's terminal draws from.
struct Source {
    screen: SharedScreen,
    /// This window sets the pane's grid.
    sizing: Option<PaneAt>,
    /// This window watches at someone else's grid and Claim would make it
    /// the one sizing: offered when that grid does not fit (#161).
    claimable: bool,
    history: bool,
}

impl Terminals {
    pub fn render_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().size_full().flex().flex_row();
        if self.settings.sessions_open {
            row = row.child(self.render_rail(window, cx));
        }
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(self.render_strip(cx))
                .child(self.render_grid(window, cx)),
        )
        .into_any_element()
    }

    fn render_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let n = self.fleet.all.len();
        let open = self.settings.sessions_open;
        let windows = self.settings.workspace.windows().len();
        let tab = div()
            .id("sessions-toggle")
            .h_full()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_color(if open { t.text } else { t.text2 })
            .when(open, |d| d.border_b_2().border_color(t.accent))
            .child("Sessions")
            .child(
                div()
                    .text_size(px(size::SMALL))
                    .text_color(t.dim)
                    .child(n.to_string()),
            )
            .on_click(cx.listener(|s, _, window, cx| {
                s.run(super::chords::Action::Sessions, window, cx);
            }));
        let layout = div()
            .ml_auto()
            .flex()
            .items_baseline()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(size::SMALL))
                    .text_color(t.dim)
                    .child("LAYOUT"),
            )
            .child(div().text_color(t.text).child(windows.to_string()))
            .child(
                div()
                    .text_size(px(size::SMALL))
                    .text_color(t.dim)
                    .child("Alt+= add window · Alt+- remove empty"),
            );
        div()
            .flex_none()
            .h(px(size::STRIP))
            .px(px(8.))
            .flex()
            .items_center()
            .bg(t.surface)
            .border_b_1()
            .border_color(t.divider)
            .child(tab)
            .child(layout)
    }

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let n = self.settings.workspace.windows().len();
        // A row or a tag in the air is the only reason a seam exists: a drop
        // there makes the next window and lands the session in it.
        let seams = cx.has_active_drag() && n < MAX_WINDOWS;
        if !seams {
            self.seam_over = None;
        }
        if !cx.has_active_drag() {
            self.dragging_over = None;
        }
        let tiles: Vec<AnyElement> = (0..n).map(|i| self.render_tile(i, window, cx)).collect();
        let mut grid = div()
            .id("grid")
            .track_focus(&self.term_focus)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(size::TILE_GAP))
            .p(px(size::TILE_GAP))
            .bg(t.raised)
            .on_key_down(cx.listener(Self::term_key))
            .on_mouse_move(cx.listener(Self::term_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::term_mouse_up))
            // A drag that ends outside the windows still copies.
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::term_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::term_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::term_mouse_up))
            .on_hover(cx.listener(|s, over: &bool, _, cx| {
                if !*over {
                    s.hover_link(None, cx);
                }
            }))
            .on_scroll(cx.listener(Self::term_scroll));
        // A press that began in a window is followed outside it, and outside
        // the app's window while the button is held.
        let entity = cx.entity();
        grid = grid.child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let entity = entity.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            entity.update(cx, |s, cx| s.term_drag(e, window, cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0(),
        );
        let mut tiles = tiles.into_iter();
        // One row of up to three, or two rows of two (CHROTE's grid-1..4).
        // Every layout that can still grow is one row, so a seam is always
        // between two windows side by side.
        let rows: Vec<usize> = if n == 4 { vec![2, 2] } else { vec![n] };
        for count in rows {
            let mut r = div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .gap(px(size::TILE_GAP));
            for k in 0..count {
                if let Some(tile) = tiles.next() {
                    let seam = (seams && k + 1 < count).then(|| self.seam(k, &t, cx));
                    r = r.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .relative()
                            .child(tile)
                            .children(seam),
                    );
                }
            }
            grid = grid.child(r);
        }
        grid
    }

    /// The seam after window `after`: the gap to the next window and a pad
    /// either side, painted over both so it takes the drop.
    fn seam(&self, after: usize, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let over = self.seam_over == Some(after);
        let reach = size::TILE_GAP + 2. * size::SEAM_PAD;
        deferred(
            div()
                .id(("seam", after))
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(-(size::TILE_GAP + size::SEAM_PAD)))
                .w(px(reach))
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(size::TILE_GAP))
                        .h_full()
                        .when(over, |d| d.bg(t.accent)),
                )
                .on_drag_move(
                    cx.listener(move |s, e: &gpui::DragMoveEvent<Dragged>, _, cx| {
                        let here = e.bounds.contains(&e.event.position);
                        if here && s.seam_over != Some(after) {
                            s.seam_over = Some(after);
                            s.dragging_over = None;
                            cx.notify();
                        } else if !here && s.seam_over == Some(after) {
                            s.seam_over = None;
                            cx.notify();
                        }
                    }),
                )
                // seam_over stays until the drag is over (render_grid), so
                // the window under the seam refuses this drop either way.
                .on_drop(cx.listener(|s, d: &Dragged, window, cx| {
                    s.dragging_over = None;
                    s.add_window_with(d.key.clone(), window, cx);
                })),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// Shows the next window and binds `key` in it (a drop on a seam).
    fn add_window_with(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.workspace.add_window() {
            let w = self.settings.workspace.focus;
            self.attach(key, w, window, cx);
        }
    }
    fn source(
        &self,
        i: usize,
        pane: u32,
        session: &parvi_terminals_contract::Session,
    ) -> Option<Source> {
        let at = PaneAt {
            node: session.node.clone(),
            pane,
        };
        if let Some(h) = self.histories.get(&i)
            && h.pane == at
        {
            return Some(Source {
                screen: h.screen.clone(),
                sizing: None,
                claimable: false,
                history: true,
            });
        }
        let screen = self.fleet.screen(&at.node, pane)?;
        // One window sizes a pane: the first in the tab that shows it.
        let first = self
            .window_panes()
            .iter()
            .position(|p| p.as_ref() == Some(&at))
            == Some(i);
        let sizing = (first
            && crate::remote::Remote::sizes_here(
                session,
                self.fleet.holds(&at.node, pane),
                self.fleet.given_back(&at.node, pane),
            ))
        .then_some(at);
        let claimable = first && sizing.is_none() && pinned(session, pane).is_none();
        Some(Source {
            screen,
            sizing,
            claimable,
            history: false,
        })
    }

    fn render_tile(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let ws = &self.settings.workspace;
        let focused = ws.focus == i;
        let binding = ws.shown(i).cloned();
        let head = self.tile_head(i, focused, &t, cx);
        let state = self.fleet.tile(binding.as_ref());
        // Over a seam, the drop is the seam's, whatever window it overlaps.
        let drop_over = self.dragging_over == Some(i) && self.seam_over.is_none();
        let frame = div()
            .id(("tile", i))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_1()
            .border_color(if focused || drop_over {
                t.accent
            } else {
                t.divider
            })
            .when(drop_over, |d| d.border_dashed())
            .overflow_hidden()
            .on_drag_move(
                cx.listener(move |s, e: &gpui::DragMoveEvent<Dragged>, _, cx| {
                    // A tag over the window it lives in has nowhere to land.
                    // Whether a seam is under the pointer is the seam's to
                    // say, whichever hears the move first.
                    let over = e.bounds.contains(&e.event.position) && e.drag(cx).lands_on(i);
                    if over && s.dragging_over != Some(i) {
                        s.dragging_over = Some(i);
                        cx.notify();
                    } else if !over && s.dragging_over == Some(i) {
                        s.dragging_over = None;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |s, d: &Dragged, window, cx| {
                s.dragging_over = None;
                if d.lands_on(i) && s.seam_over.is_none() {
                    s.attach(d.key.clone(), i, window, cx);
                }
            }));
        let body: AnyElement = match state {
            TileState::Empty => {
                let launcher = self.launchers[i].clone();
                let grids = self.launch_grids.clone();
                let font = self.settings.font_size;
                // The grid a terminal here would hold: its box less the pad.
                let measure = canvas(
                    |_, _, _| (),
                    move |b, _, window, _| {
                        let pad = px(2. * size::TERM_PAD);
                        let inner = gpui::size(b.size.width - pad, b.size.height - pad);
                        let grid = paint::grid_for(inner, paint::metrics(window, font));
                        grids.borrow_mut()[i] = Some(grid);
                    },
                )
                .absolute()
                .inset_0();
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .bg(t.term_bg)
                    .child(measure)
                    .child(
                        div()
                            .id(("launcher-scroll", i))
                            .absolute()
                            .inset_0()
                            .overflow_y_scroll()
                            .p(px(12.))
                            .flex()
                            .flex_col()
                            .items_center()
                            // A fixed share of the window above the panel, not
                            // centring: the panel keeps its top edge when the
                            // node it targets lists more or fewer harnesses.
                            .child(div().h(relative(size::LAUNCHER_TOP)).min_h(px(0.)))
                            .child(launcher)
                            .child(
                                div()
                                    .pt(px(14.))
                                    .text_size(px(12.))
                                    .italic()
                                    .text_color(t.dim)
                                    .child("or drag a session here"),
                            )
                            .child(div().flex_1().min_h(px(0.))),
                    )
                    .into_any_element()
            }
            TileState::Live { session, pane } => {
                let session = session.clone();
                match self.source(i, pane, &session) {
                    Some(src) => self.term_body(i, src, focused, &t, window, cx),
                    None => self.detached_body(
                        None,
                        kit::note(&t, "Connecting…", None).into_any_element(),
                        &t,
                    ),
                }
            }
            TileState::Connecting(_) => self.detached_body(
                None,
                kit::note(&t, "Connecting…", None).into_any_element(),
                &t,
            ),
            TileState::Lost(_) => {
                let last = self.bound_pane_screen(i);
                let node = self.window_node(i);
                let device = node
                    .as_deref()
                    .and_then(|n| self.where_is(n))
                    .map(|p| p.device.to_uppercase())
                    .unwrap_or_else(|| "the node".into());
                let since = node
                    .as_deref()
                    .and_then(|n| self.away_since(n))
                    .map(|at| format!("Away since {at}. "))
                    .unwrap_or_default();
                let note = div()
                    .px(px(14.))
                    .py(px(8.))
                    .rounded(px(size::RADIUS))
                    .bg(alpha(t.surface, 0.85))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("Waiting for {device}")),
                    )
                    .child(kit::note(
                        &t,
                        format!("{since}The session is kept; it comes back by itself."),
                        Some(t.dim),
                    ));
                self.detached_body(last, note.into_any_element(), &t)
            }
            TileState::Ended(name) => {
                let note = format!("{name} has ended.");
                let last = binding
                    .as_ref()
                    .and_then(|b| self.fleet.ended_screen(&b.key, b.pane));
                let key = binding.clone().map(|b| b.key).unwrap_or_default();
                let restart = if self.restarting.contains(&key) {
                    "Restarting…"
                } else {
                    "Restart"
                };
                let panel = self.ended_panel(
                    i,
                    &note,
                    &[
                        (restart, Act::Restart(key.clone())),
                        ("Remove", Act::Remove(i, key)),
                    ],
                    &t,
                    cx,
                );
                self.detached_body(last, panel, &t)
            }
        };
        // Only the body focuses the window and reaches the terminal; the
        // head is the tags'.
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s, e: &MouseDownEvent, window, cx| {
                    s.tile_mouse_down(i, e, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |s, e: &MouseDownEvent, window, cx| {
                    s.tile_mouse_down(i, e, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |s, e: &MouseDownEvent, window, cx| {
                    s.tile_mouse_down(i, e, window, cx)
                }),
            )
            .child(body)
            .when(drop_over, |d| d.child(drop_note(&t)));
        frame.child(head).child(body).into_any_element()
    }

    fn bound_pane_screen(&self, i: usize) -> Option<SharedScreen> {
        let b = self.settings.workspace.shown(i)?;
        let s = self.fleet.session(&b.key)?;
        let p = s
            .panes
            .iter()
            .find(|p| Some(p.id) == b.pane)
            .or(s.panes.first())?;
        self.fleet.screen(&b.key.node, p.id)
    }

    fn window_node(&self, i: usize) -> Option<String> {
        Some(self.settings.workspace.shown(i)?.key.node.clone())
    }

    /// The passport of the node window `i` shows a session of.
    fn passport_here(&self, i: usize, t: &Theme) -> Option<gpui::Div> {
        let p = self.where_is(&self.window_node(i)?)?;
        Some(place::passport(t, &p.device, &p.environment))
    }

    /// A window's head: its device's passport, a tag for each session bound
    /// to it, and Send and the menu for the one on screen. A right click
    /// anywhere on it opens that session's menu.
    fn tile_head(
        &mut self,
        i: usize,
        focused: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = self.settings.workspace.windows()[i].clone();
        let mut tags = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_end()
            .gap(px(2.))
            .overflow_hidden();
        for (n, b) in w.sessions.iter().enumerate() {
            tags = tags.child(self.tag(i, n, b, n == w.active, focused, t, cx));
        }
        let mut head = self
            .head_frame(t)
            .id(("head", i))
            .children(self.passport_here(i, t))
            .child(tags);
        let Some(shown) = w.shown().cloned() else {
            return head.into_any_element();
        };
        if let TileState::Live { .. } = self.fleet.tile(Some(&shown)) {
            let send_key = shown.key.clone();
            let menu_key = shown.key.clone();
            head =
                head.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .pb(px(3.))
                        .child(kit::button(t, ("send", i), "Send", Kind::Quiet).on_click(
                            cx.listener(move |s, _, window, cx| {
                                s.open_send(Some(send_key.clone()), window, cx);
                            }),
                        ))
                        .child(kit::button(t, ("menu", i), "…", Kind::Quiet).on_click(
                            cx.listener(move |s, e: &gpui::ClickEvent, window, cx| {
                                let items = s.session_menu(i, &menu_key);
                                s.menu(items, e.position(), window, cx);
                            }),
                        )),
                );
        }
        let key = shown.key;
        head.on_mouse_down(
            MouseButton::Right,
            cx.listener(move |s, e: &MouseDownEvent, window, cx| {
                let items = s.session_menu(i, &key);
                s.menu(items, e.position, window, cx);
            }),
        )
        .into_any_element()
    }

    /// One session's tag: pressed it goes on screen, dragged it moves to
    /// another window or a seam, × takes it out of the window.
    #[allow(clippy::too_many_arguments)]
    fn tag(
        &mut self,
        i: usize,
        n: usize,
        b: &Binding,
        active: bool,
        focused: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = b.key.clone();
        let state = self.fleet.tile(Some(b));
        let session = match &state {
            TileState::Live { session, .. } => Some((*session).clone()),
            _ => None,
        };
        let word = if self.restarting.contains(&key) {
            Some("restarting")
        } else {
            match state {
                TileState::Live { .. } | TileState::Empty => None,
                TileState::Connecting(_) => Some("connecting"),
                TileState::Lost(_) => Some("offline"),
                TileState::Ended(_) => Some("ended"),
            }
        };
        let seat = session.as_ref().and_then(|s| match s.seat {
            Some(parvi_terminals_contract::Seat::Mine) => None,
            Some(parvi_terminals_contract::Seat::Other) => Some("viewing"),
            None if s.foreign.unwrap_or(0) > 0 || s.pinned == Some(true) => Some("viewing"),
            None => None,
        });
        let history = active && self.histories.contains_key(&i);
        // Elided here by characters, keeping the tail; the label itself
        // never shrinks, since a clip would cut the tail instead.
        let label = sessions::elide_head(&key.name, if active { 24 } else { 12 });
        let id = i * 100 + n;
        let (k_show, k_menu, k_remove) = (key.clone(), key.clone(), key.clone());
        let name = key.name.clone();
        div()
            .id(("tag", id))
            .flex_shrink(1.)
            .min_w(px(0.))
            .max_w(px(size::TAG_MAX))
            .h(px(28.))
            .pl(px(8.))
            .pr(px(2.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded_t(px(6.))
            .cursor_pointer()
            .bg(if active && focused {
                t.surface
            } else if active {
                t.raised
            } else {
                gpui::transparent_black()
            })
            .when(active, |d| d.border_t_2().border_color(t.accent))
            .text_color(if active { t.text } else { t.text2 })
            .hover(|d| d.text_color(t.text))
            .when_some(session.as_ref().and_then(|s| s.user.clone()), |d, u| {
                d.child(kit::user_square(t, &u))
            })
            .when(
                session
                    .as_ref()
                    .is_some_and(|s| sessions::waiting(s).is_some()),
                |d| d.child(kit::waiting_dot(t)),
            )
            // What it runs, as its rail row says: nothing for a shell.
            .children(
                session
                    .as_ref()
                    .and_then(|s| super::rail::running_mark(t, s, cx)),
            )
            .child(div().flex_none().whitespace_nowrap().child(label))
            // What a terminal does not show is said for the one on screen;
            // in a narrow window the badges give way, never the name or ×.
            .children(session.iter().filter(|_| active).map(|s| {
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .children(
                        sessions::badges(s)
                            .into_iter()
                            .enumerate()
                            .map(|(m, b)| kit::badge(t, ("badge", id * 10 + m), b.text, b.why)),
                    )
            }))
            .children(word.or(seat).map(|w| {
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(t.dim)
                    .child(w.to_uppercase())
            }))
            .when(history, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(t.accent)
                        .child("HISTORY"),
                )
            })
            .child(
                div()
                    .id(("tag-remove", id))
                    .flex_none()
                    .px(px(4.))
                    .text_color(t.dim)
                    .hover(|d| d.text_color(t.text))
                    .child("×")
                    // The remove control is not the tag: it neither shows
                    // the session nor starts a drag.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |s, _, window, cx| {
                        s.act(Act::Remove(i, k_remove.clone()), window, cx)
                    })),
            )
            // The press shows it, so a click that drifts into a drag still
            // does; only the primary button, the other one is the menu's.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s, _, window, cx| {
                    if !s
                        .settings
                        .workspace
                        .shown(i)
                        .is_some_and(|b| b.key == k_show)
                    {
                        s.act(Act::ShowTag(i, k_show.clone()), window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |s, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let items = s.session_menu(i, &k_menu);
                    s.menu(items, e.position, window, cx);
                }),
            )
            .on_drag(Dragged { key, from: Some(i) }, move |_, _, _, cx| {
                cx.new(|_| DragPreview::named(name.clone()))
            })
            .into_any_element()
    }

    fn head_frame(&self, t: &Theme) -> gpui::Div {
        div()
            .flex_none()
            .h(px(size::TILE_HEAD))
            .px(px(8.))
            .pt(px(4.))
            .flex()
            .items_end()
            .gap(px(8.))
            .bg(t.raised)
            .border_b_1()
            .border_color(t.divider)
    }

    /// The menu of a session bound in window `i`: the full set while it is on
    /// screen and live, else what acts on the session itself.
    pub fn session_menu(&self, i: usize, key: &Key) -> Vec<MenuItem> {
        let shown = self
            .settings
            .workspace
            .shown(i)
            .is_some_and(|b| &b.key == key);
        let pane = self.window_panes().get(i).cloned().flatten();
        if shown && let Some(at) = pane {
            return self.tile_menu(i, key, at.pane);
        }
        let mut items = vec![];
        if !shown {
            items.push(MenuItem::new(
                "Show in this window",
                Act::ShowTag(i, key.clone()),
            ));
        }
        let live = self.fleet.session(key).is_some();
        if live {
            items.push(MenuItem::new("Send…", Act::Send(key.clone())).chord("Alt+S"));
            items.push(MenuItem::new("Peek", Act::Peek(key.clone())));
        } else if matches!(
            self.fleet.tile(Some(&Binding {
                key: key.clone(),
                pane: None
            })),
            TileState::Ended(_)
        ) {
            items.push(MenuItem::new("Restart", Act::Restart(key.clone())));
        }
        items.push(MenuItem::rule());
        if live {
            items.push(MenuItem::new("Rename session…", Act::Rename(key.clone())));
            items.push(self.copy_row(key));
        }
        items.push(MenuItem::new(
            "Remove from window",
            Act::Remove(i, key.clone()),
        ));
        if live {
            items.push(MenuItem::new("Kill session", Act::Kill(key.clone())).danger());
        }
        items
    }

    pub fn tile_menu(&self, i: usize, key: &Key, pane: u32) -> Vec<MenuItem> {
        let mut items = vec![
            MenuItem::new("Send…", Act::Send(key.clone())).chord("Alt+S"),
            MenuItem::new("Peek", Act::Peek(key.clone())),
            MenuItem::new("Scroll back", Act::History(i)),
        ];
        let session = self.fleet.session(key);
        if self.fleet.holds(&key.node, pane) {
            items.push(MenuItem::new("Give the size back", Act::Release(i)));
        } else if let Some(why) = session.and_then(|s| pinned(s, pane)) {
            items.push(MenuItem::new("Claim the size", Act::Claim(i)).refused(why));
        } else {
            items.push(
                MenuItem::new("Claim the size", Act::Claim(i))
                    .chord("Alt+C")
                    .hint(CLAIM_EXPLANATION),
            );
        }
        if session.is_some_and(|s| s.windows.unwrap_or(0) > 1) {
            items.push(MenuItem::new(
                "tmux windows…",
                Act::WindowList(i, key.clone()),
            ));
        }
        if let Some(s) = session
            && s.panes.len() > 1
        {
            items.push(MenuItem::rule());
            for (n, p) in s.panes.iter().enumerate() {
                items.push(
                    MenuItem::new(
                        format!("Pane {} ({}×{})", n + 1, p.cols, p.rows),
                        Act::Pane(i, p.id),
                    )
                    .checked(p.id == pane),
                );
            }
        }
        items.push(MenuItem::rule());
        items.push(MenuItem::new("Rename session…", Act::Rename(key.clone())));
        items.push(self.copy_row(key));
        items.push(MenuItem::new(
            "Remove from window",
            Act::Remove(i, key.clone()),
        ));
        items.push(MenuItem::new("Kill session", Act::Kill(key.clone())).danger());
        items
    }
    /// The session's tmux windows, as a menu where the last one stood,
    /// once its node has listed them.
    pub(super) fn window_list(&mut self, i: usize, key: Key, cx: &mut Context<Self>) {
        let Some(session) = self.fleet.session(&key) else {
            return;
        };
        let path = format!(
            "/api/terminal/windows?session={}",
            crate::http::enc(&session.id)
        );
        let selected = self
            .window_panes()
            .get(i)
            .cloned()
            .flatten()
            .map(|p| p.pane);
        let task = self
            .host
            .request(Request::get(NodeId(key.node.clone()), path), cx);
        cx.spawn(async move |this, cx| {
            let items = match crate::http::json::<Vec<TmuxWindow>>(task.await) {
                Ok(windows) if !windows.is_empty() => windows
                    .into_iter()
                    .map(|w| {
                        MenuItem::new(
                            format!("{}: {}", w.index, w.name),
                            Act::WindowPane(i, key.clone(), w.pane),
                        )
                        .checked(Some(w.pane) == selected)
                    })
                    .collect(),
                Ok(_) => vec![MenuItem::note("No tmux windows found")],
                Err(e) => vec![MenuItem::note(format!("Could not load windows: {e}"))],
            };
            let _ = this.update(cx, |s, cx| {
                let rows = items.into_iter().map(|i| i.row(&s.me)).collect();
                s.host.menu(rows, s.menu_at, cx);
            });
        })
        .detach();
    }

    /// The buttons an ended window offers, on a panel.
    fn ended_panel(
        &mut self,
        i: usize,
        note: &str,
        actions: &[(&str, Act)],
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut buttons = div().flex().gap(px(8.)).justify_center();
        for (n, (label, act)) in actions.iter().enumerate() {
            let act = act.clone();
            let kind = if n == 0 { Kind::Primary } else { Kind::Plain };
            buttons = buttons.child(
                kit::button(t, ("detached", i * 10 + n), label.to_string(), kind)
                    .on_click(cx.listener(move |s, _, window, cx| s.act(act.clone(), window, cx))),
            );
        }
        kit::panel(t)
            .px(px(18.))
            .py(px(14.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .max_w(px(360.))
            .child(kit::note(t, note.to_string(), None))
            .child(buttons)
            .into_any_element()
    }

    /// A window without a live terminal: the last frame dimmed behind, and
    /// what to know over it.
    fn detached_body(&self, last: Option<SharedScreen>, over: AnyElement, t: &Theme) -> AnyElement {
        let theme_t = t.clone();
        let behind = last.map(|screen| {
            let font = self.settings.font_size;
            div()
                .absolute()
                .top(px(size::TERM_PAD))
                .left(px(size::TERM_PAD))
                .right(px(size::TERM_PAD))
                .bottom(px(size::TERM_PAD))
                .opacity(crate::theme::fade::DIMMED)
                .child(
                    canvas(
                        |_, _, _| (),
                        move |b, _, window, cx| {
                            let s = screen.lock();
                            let m = paint::fit(window, b.size, s.cols, s.rows, font);
                            let o = paint::origin(b, m, s.rows_to_show());
                            window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                                paint::paint(
                                    &s,
                                    m,
                                    o,
                                    Look {
                                        theme: &theme_t,
                                        cursor: false,
                                        focused: false,
                                        link: &[],
                                    },
                                    window,
                                    cx,
                                );
                            });
                        },
                    )
                    .size_full(),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .bg(t.term_bg)
            .children(behind)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(over),
            )
            .into_any_element()
    }

    fn term_body(
        &mut self,
        i: usize,
        src: Source,
        focused: bool,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let geo = self.geo.clone();
        let focus = self.term_focus.clone();
        let font = self.settings.font_size;
        let theme_t = t.clone();
        let take_input = focused && self.surfaces.is_empty();
        let hover = self
            .link_hover
            .filter(|h| h.0 == i)
            .map(|(_, col, row)| (col, row));
        let Source {
            screen,
            sizing,
            claimable,
            history,
        } = src;
        // Below the floor an observer clips; it offers to size the session
        // to this window instead (#161). Judged from the last paint's box.
        let offer = claimable
            && self.geo.borrow()[i].body.is_some_and(|b| {
                let s = screen.lock();
                let m = paint::fit(window, b.size, s.cols, s.rows, font);
                !paint::fits(m, b.size, s.cols, s.rows)
            });
        let body = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                if take_input {
                    window.handle_input(
                        &focus,
                        ElementInputHandler::new(bounds, entity.clone()),
                        cx,
                    );
                }
                let mut s = screen.lock();
                let m = if sizing.is_some() {
                    paint::metrics(window, font)
                } else {
                    paint::fit(window, bounds.size, s.cols, s.rows, font)
                };
                if let Some(at) = &sizing {
                    let (cols, rows) = paint::grid_for(bounds.size, m);
                    entity.update(cx, |sh, cx| {
                        sh.fleet.claim(&at.node, at.pane, cols, rows);
                        sh.flush(cx);
                    });
                }
                let o = paint::origin(bounds, m, s.rows_to_show());
                // Found again at each paint, so output that moves the text
                // under a resting pointer moves the underline with it.
                let link = hover
                    .and_then(|(col, row)| {
                        let line = Line(row as i32) - s.term.grid().display_offset();
                        links::at(&s.term, GridPoint::new(line, Column(col)))
                    })
                    .map(|l| l.cells)
                    .unwrap_or_default();
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    paint::paint(
                        &s,
                        m,
                        o,
                        Look {
                            theme: &theme_t,
                            cursor: !history,
                            focused,
                            link: &link,
                        },
                        window,
                        cx,
                    );
                });
                s.dirty = false;
                geo.borrow_mut()[i] = Geo {
                    body: Some(bounds),
                    origin: Some(o),
                    cell: Some(m.cell),
                    rows: s.rows,
                    cols: s.cols,
                    display_offset: s.term.grid().display_offset(),
                };
            },
        )
        .size_full();
        let hint = history.then(|| {
            div()
                .absolute()
                .bottom(px(8.))
                .right(px(12.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(3.))
                .bg(alpha(t.surface, 0.92))
                .border_1()
                .border_color(t.divider)
                .text_size(px(11.))
                .text_color(t.text2)
                .child("History · type or scroll to the bottom to return")
        });
        let claim = offer.then(|| self.clipped_note(i, t, cx));
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .bg(t.term_bg)
            .p(px(size::TERM_PAD))
            .cursor(if hover.is_some() {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::IBeam
            })
            .child(body)
            .children(hint)
            .children(claim)
            .into_any_element()
    }

    /// What an observer clipped past the floor says, over its bottom edge:
    /// the grid is too big to read here, and Claim makes it fit.
    fn clipped_note(&self, i: usize, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .absolute()
            .bottom(px(8.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                kit::panel(t)
                    .px(px(10.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .child("Too large to read here, so it is cut off.")
                    .child(
                        kit::button(t, ("clipped-claim", i), "Claim the size", Kind::Primary)
                            .tooltip(kit::tip(CLAIM_EXPLANATION))
                            .on_click(cx.listener(move |s, _, window, cx| {
                                s.act(Act::Claim(i), window, cx)
                            })),
                    ),
            )
    }

    // ---- input to the terminal ----

    /// The screen a window shows now: its history, or its live pane.
    /// What window `i` shows now, line by line, as the operator reads it.
    pub fn window_text(&self, i: usize) -> Option<String> {
        Some(self.shown_screen(i)?.0.lock().text())
    }

    fn shown_screen(&self, i: usize) -> Option<(SharedScreen, Option<PaneAt>)> {
        if let Some(h) = self.histories.get(&i) {
            return Some((h.screen.clone(), None));
        }
        let at = self.window_panes().get(i).cloned().flatten()?;
        Some((self.fleet.screen(&at.node, at.pane)?, Some(at)))
    }

    fn focused_pane(&self) -> Option<PaneAt> {
        self.window_panes()
            .get(self.settings.workspace.focus)
            .cloned()
            .flatten()
    }

    /// Bytes for the focused window's program. Leaves history first.
    pub fn send_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let w = self.settings.workspace.focus;
        if self.histories.remove(&w).is_some() {
            cx.notify();
        }
        let Some(pane) = self.focused_pane() else {
            return;
        };
        if let Some(s) = self.fleet.screen(&pane.node, pane.pane) {
            let mut s = s.lock();
            s.term.selection = None;
            if s.scrolled_back() {
                s.term.scroll_display(Scroll::Bottom);
            }
        }
        self.fleet.input(&pane.node, pane.pane, bytes);
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let bracketed = self
            .fleet
            .screen(&pane.node, pane.pane)
            .is_some_and(|s| s.lock().mode().contains(TermMode::BRACKETED_PASTE));
        self.send_input(&keys::paste(&text, bracketed), cx);
    }

    fn copy_selection(&mut self, i: usize, cx: &mut Context<Self>) -> bool {
        let Some((screen, _)) = self.shown_screen(i) else {
            return false;
        };
        let text = screen.lock().term.selection_to_string();
        match text {
            Some(t) if !t.is_empty() => {
                self.copy_text(t, cx);
                true
            }
            _ => false,
        }
    }

    /// Puts a selection on the clipboard and says whether it got there. The
    /// platform's write reports nothing (Windows fails when another program
    /// holds the clipboard open), so the clipboard is read back.
    fn copy_text(&mut self, text: String, cx: &mut Context<Self>) {
        let text = keys::copied(text);
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        let landed = cx.read_from_clipboard().and_then(|c| c.text());
        if landed.as_deref() == Some(text.as_str()) {
            self.toast("Copied", cx);
        } else {
            self.report(
                "Could not copy the selection: the clipboard did not take it",
                true,
                None,
            );
            cx.notify();
        }
    }

    pub fn term_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let w = self.settings.workspace.focus;
        if m.control && m.shift && k.key == "c" {
            self.copy_selection(w, cx);
            cx.stop_propagation();
            return;
        }
        if (m.control && !m.alt && k.key == "v") || (m.shift && !m.control && k.key == "insert") {
            self.paste_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        // Shift+PageUp/PageDown read the history, as terminals do.
        if m.shift && !m.control && (k.key == "pageup" || k.key == "pagedown") {
            self.scroll_window(w, if k.key == "pageup" { 20 } else { -20 }, cx);
            cx.stop_propagation();
            return;
        }
        if e.prefer_character_input {
            return;
        }
        let app_cursor = self
            .shown_screen(w)
            .is_some_and(|(s, _)| s.lock().mode().contains(TermMode::APP_CURSOR));
        if let Some(b) = keys::bytes(k, app_cursor) {
            self.send_input(&b, cx);
            cx.stop_propagation();
        }
    }

    fn window_at(&self, p: gpui::Point<Pixels>) -> Option<usize> {
        self.geo
            .borrow()
            .iter()
            .position(|g| g.body.is_some_and(|b| b.contains(&p)))
            .filter(|i| *i < self.settings.workspace.windows().len())
    }

    fn cell_at(&self, i: usize, p: gpui::Point<Pixels>) -> Option<(usize, usize, Side)> {
        let g = self.geo.borrow()[i];
        let (o, cell) = (g.origin?, g.cell?);
        let x = f32::from(p.x - o.x) / f32::from(cell.width);
        let y = f32::from(p.y - o.y) / f32::from(cell.height);
        let col = (x.max(0.) as usize).min(g.cols.saturating_sub(1));
        let row = (y.max(0.) as usize).min(g.rows.saturating_sub(1));
        Some((
            col,
            row,
            if x.fract() < 0.5 {
                Side::Left
            } else {
                Side::Right
            },
        ))
    }

    fn grid_point(&self, i: usize, p: gpui::Point<Pixels>) -> Option<(GridPoint, Side)> {
        let (col, row, side) = self.cell_at(i, p)?;
        let offset = self.geo.borrow()[i].display_offset;
        Some((GridPoint::new(Line(row as i32) - offset, Column(col)), side))
    }

    /// The program asked for the mouse, and Shift isn't overriding it.
    fn mouse_mode(&self, i: usize, shift: bool) -> Option<(TermMode, PaneAt)> {
        if shift || self.histories.contains_key(&i) {
            return None;
        }
        let (screen, pane) = self.shown_screen(i)?;
        let mode = screen.lock().mode();
        mode.intersects(TermMode::MOUSE_MODE)
            .then_some((mode, pane?))
    }

    fn tile_mouse_down(
        &mut self,
        i: usize,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.workspace.focus != i {
            self.settings.workspace.focus = i;
            self.save(cx);
            cx.notify();
        }
        let in_body = self.geo.borrow()[i]
            .body
            .is_some_and(|b| b.contains(&e.position));
        if !in_body || self.window_panes().get(i).cloned().flatten().is_none() {
            return;
        }
        window.focus(&self.term_focus, cx);
        let button = match e.button {
            MouseButton::Left => mouse::Button::Left,
            MouseButton::Middle => mouse::Button::Middle,
            MouseButton::Right => mouse::Button::Right,
            _ => return,
        };
        if let Some((mode, pane)) = self.mouse_mode(i, e.modifiers.shift)
            && let Some((col, row, _)) = self.cell_at(i, e.position)
        {
            let mods = (e.modifiers.shift, e.modifiers.alt, e.modifiers.control);
            self.fleet.input(
                &pane.node,
                pane.pane,
                &mouse::report(mode, button, true, false, col, row, mods),
            );
            self.mouse_down_in = Some((i, button as u8));
            cx.stop_propagation();
            return;
        }
        match e.button {
            MouseButton::Left => {
                let Some((p, side)) = self.grid_point(i, e.position) else {
                    return;
                };
                let ty = match e.click_count {
                    2 => SelectionType::Semantic,
                    3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                if let Some((screen, _)) = self.shown_screen(i) {
                    screen.lock().term.selection = Some(Selection::new(ty, p, side));
                }
                self.selecting = Some(i);
                cx.notify();
            }
            MouseButton::Middle => self.paste_clipboard(cx),
            MouseButton::Right => {
                // A right click copies a selection, else offers the menu.
                if !self.copy_selection(i, cx)
                    && let (Some(b), Some(at)) = (
                        self.settings.workspace.shown(i).cloned(),
                        self.window_panes()[i].clone(),
                    )
                {
                    let items = self.tile_menu(i, &b.key, at.pane);
                    self.menu(items, e.position, window, cx);
                }
            }
            _ => {}
        }
    }

    fn term_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // A press from a window is followed everywhere (`term_drag`).
        if self.selecting.is_some() || self.mouse_down_in.is_some() {
            return;
        }
        let Some(i) = self.window_at(e.position) else {
            self.hover_link(None, cx);
            return;
        };
        let held = e.pressed_button.is_some();
        let program = self.mouse_mode(i, e.modifiers.shift);
        // A link shows where the mouse is the reader's: no button held, and
        // the program not listening to it (or Shift overriding it).
        let link = (program.is_none() && !held)
            .then(|| self.link_at(i, e.position))
            .flatten()
            .map(|(col, row, _)| (i, col, row));
        self.hover_link(link, cx);
        if let Some((mode, pane)) = program
            && mouse::wants(mode, true, held)
            && let Some((col, row, _)) = self.cell_at(i, e.position)
        {
            let button = match e.pressed_button {
                Some(MouseButton::Left) => mouse::Button::Left,
                Some(MouseButton::Middle) => mouse::Button::Middle,
                Some(MouseButton::Right) => mouse::Button::Right,
                _ => mouse::Button::None,
            };
            let mods = (e.modifiers.shift, e.modifiers.alt, e.modifiers.control);
            self.fleet.input(
                &pane.node,
                pane.pane,
                &mouse::report(mode, button, true, true, col, row, mods),
            );
        }
    }

    fn hover_link(&mut self, at: Option<(usize, usize, usize)>, cx: &mut Context<Self>) {
        if self.link_hover != at {
            self.link_hover = at;
            cx.notify();
        }
    }

    /// The link drawn where the pointer is in window `i`: its cell on
    /// screen, and the link.
    fn link_at(&self, i: usize, p: gpui::Point<Pixels>) -> Option<(usize, usize, Link)> {
        let g = self.geo.borrow()[i];
        let (o, cell) = (g.origin?, g.cell?);
        let grid = Bounds::new(
            o,
            gpui::size(cell.width * g.cols as f32, cell.height * g.rows as f32),
        );
        if !g.body.is_some_and(|b| b.contains(&p)) || !grid.contains(&p) {
            return None;
        }
        let (col, row, _) = self.cell_at(i, p)?;
        let (screen, _) = self.shown_screen(i)?;
        let s = screen.lock();
        let line = Line(row as i32) - s.term.grid().display_offset();
        links::at(&s.term, GridPoint::new(line, Column(col))).map(|l| (col, row, l))
    }

    /// A link was clicked: an address opens in the browser, a path in the
    /// file viewer beside, on the device of the window's session.
    fn open_link(&mut self, i: usize, target: links::Target, cx: &mut Context<Self>) {
        match target {
            links::Target::Url(url) => cx.open_url(&url),
            links::Target::Path(path) => {
                let node = match self.histories.get(&i) {
                    Some(h) => Some(h.pane.node.clone()),
                    None => self
                        .window_panes()
                        .get(i)
                        .cloned()
                        .flatten()
                        .map(|a| a.node),
                };
                if let Some(node) = node {
                    let ask = Ask::OpenPath {
                        node: NodeId(node),
                        path,
                        beside: true,
                    };
                    if let Err(e) = self.host.ask(ask, cx) {
                        self.report(&e.message, true, None);
                    }
                }
            }
        }
    }

    fn term_mouse_up(&mut self, e: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((i, b)) = self.mouse_down_in.take() {
            if let Some((mode, pane)) = self.mouse_mode(i, false)
                && let Some((col, row, _)) = self.cell_at(i, e.position)
            {
                let button = match b {
                    0 => mouse::Button::Left,
                    1 => mouse::Button::Middle,
                    _ => mouse::Button::Right,
                };
                let mods = (e.modifiers.shift, e.modifiers.alt, e.modifiers.control);
                self.fleet.input(
                    &pane.node,
                    pane.pane,
                    &mouse::report(mode, button, false, false, col, row, mods),
                );
            }
            return;
        }
        let Some(i) = self.selecting.take() else {
            return;
        };
        self.end_drag();
        let Some((screen, _)) = self.shown_screen(i) else {
            return;
        };
        let text = screen.lock().term.selection_to_string();
        match text {
            Some(t) if !t.is_empty() => {
                if self.settings.copy_on_select {
                    self.copy_text(t, cx);
                }
            }
            // A plain click selects nothing, copies nothing, and opens the
            // link it lands on.
            _ => {
                screen.lock().term.selection = None;
                if let Some((_, _, link)) = self.link_at(i, e.position) {
                    self.open_link(i, link.target, cx);
                }
            }
        }
        cx.notify();
    }

    fn term_scroll(&mut self, e: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.window_at(e.position) else {
            return;
        };
        let cell_h = self.geo.borrow()[i]
            .cell
            .map(|c| c.height)
            .unwrap_or(px(16.));
        let lines = match e.delta {
            ScrollDelta::Lines(l) => l.y * 3.,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(cell_h),
        };
        let n = lines.round() as i32;
        if n == 0 {
            return;
        }
        // The program's own wheel first, then the alternate screen's arrows.
        if let Some((mode, pane)) = self.mouse_mode(i, e.modifiers.shift)
            && let Some((col, row, _)) = self.cell_at(i, e.position)
        {
            let b = if n > 0 {
                mouse::Button::WheelUp
            } else {
                mouse::Button::WheelDown
            };
            let mods = (e.modifiers.shift, e.modifiers.alt, e.modifiers.control);
            let r = mouse::report(mode, b, true, false, col, row, mods);
            for _ in 0..n.unsigned_abs() {
                self.fleet.input(&pane.node, pane.pane, &r);
            }
            return;
        }
        if !self.histories.contains_key(&i)
            && let Some((screen, Some(pane))) = self.shown_screen(i)
        {
            let mode = screen.lock().mode();
            if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                let app = mode.contains(TermMode::APP_CURSOR);
                let key = match (n > 0, app) {
                    (true, true) => b"\x1bOA".as_slice(),
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                for _ in 0..n.unsigned_abs() {
                    self.fleet.input(&pane.node, pane.pane, key);
                }
                return;
            }
        }
        self.scroll_window(i, n, cx);
    }

    /// Scrolls a window's history: opens it on the way up, leaves it at the
    /// bottom on the way down.
    fn scroll_window(&mut self, i: usize, n: i32, cx: &mut Context<Self>) {
        match self.histories.get(&i) {
            Some(h) => {
                let at_bottom = {
                    let mut s = h.screen.lock();
                    s.term.scroll_display(Scroll::Delta(n));
                    s.term.grid().display_offset() == 0
                };
                if at_bottom && n < 0 {
                    self.histories.remove(&i);
                }
                cx.notify();
            }
            None if n > 0 => self.open_history(i, cx),
            None => {}
        }
    }
}

/// The terminal's input method: committed text goes to the focused pane.
impl EntityInputHandler for Terminals {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_input(text.as_bytes(), cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The candidate window opens at the focused terminal's cursor.
        let w = self.settings.workspace.focus;
        let g = self.geo.borrow()[w];
        let (o, cell) = (g.origin?, g.cell?);
        let (screen, _) = self.shown_screen(w)?;
        let c = screen.lock().term.grid().cursor.point;
        Some(Bounds::new(
            point(
                o.x + cell.width * c.column.0 as f32,
                o.y + cell.height * c.line.0 as f32,
            ),
            cell,
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_terminals_contract::{Pane, Session};

    #[test]
    fn a_drop_lands_only_where_it_would_change_something() {
        let key = Key {
            node: "n".into(),
            user: String::new(),
            name: "a".into(),
        };
        let tag = Dragged {
            key: key.clone(),
            from: Some(1),
        };
        assert!(!tag.lands_on(1));
        assert!(tag.lands_on(0));
        let row = Dragged { key, from: None };
        assert!((0..MAX_WINDOWS).all(|i| row.lands_on(i)));
    }

    #[test]
    fn claim_on_a_pinned_window_says_tmuxs_size_and_why() {
        let mut s = Session {
            name: "build".into(),
            panes: vec![Pane {
                id: 4,
                cols: 100,
                rows: 30,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(pinned(&s, 4), None);
        s.pinned = Some(true);
        assert_eq!(
            pinned(&s, 4).as_deref(),
            Some(
                "Pinned at 100x30. tmux window-size is manual on this window, \
                 so Parvi cannot resize it."
            )
        );
    }
}

#[derive(serde::Deserialize)]
struct TmuxWindow {
    index: u32,
    name: String,
    pane: u32,
}
