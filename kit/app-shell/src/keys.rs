//! The keys panel: every chord live now, searchable, each one a click
//! away. It opens with the leader or its own chord, as a layer over the
//! window.

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, MouseButton, Subscription, Window, div,
    prelude::*, px,
};
use parvi_app_kit::Host;
use parvi_app_kit::chords::{self, Chord, Row};
use parvi_app_kit::theme::{size, theme};
use parvi_app_kit::widgets::field::{FieldEvent, TextField};
use parvi_app_kit::widgets::kit;

/// The panel's layer on the stack.
pub const LAYER: &str = "keys";
/// The panel, and its columns: the keys, the arrow, then the label.
const PANEL_W: f32 = 540.;
const CAPS_W: f32 = 170.;
const ARROW_W: f32 = 30.;

pub struct KeysPanel {
    host: Host,
    search: Entity<TextField>,
    chords: Vec<Chord>,
    cursor: usize,
    /// The leader waits for its key.
    leader: bool,
    _search: Subscription,
}

impl KeysPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextField::new(cx, "Search keybindings"));
        let sub = cx.subscribe_in(
            &search,
            window,
            |k: &mut Self, _, ev: &FieldEvent, window, cx| {
                let n = k.rows(cx).len();
                match ev {
                    FieldEvent::Changed => k.cursor = 0,
                    FieldEvent::Down => k.cursor = (k.cursor + 1).min(n.saturating_sub(1)),
                    FieldEvent::Up => k.cursor = k.cursor.saturating_sub(1),
                    FieldEvent::Submit => {
                        if let Some(row) = k.rows(cx).get(k.cursor).cloned() {
                            k.pick(&row, window, cx);
                        }
                    }
                    _ => {}
                }
                cx.notify();
            },
        );
        Self {
            host,
            search,
            chords: vec![],
            cursor: 0,
            leader: false,
            _search: sub,
        }
    }

    /// The chords it lists; `fresh` clears the search.
    pub fn set_chords(&mut self, chords: Vec<Chord>, fresh: bool, cx: &mut Context<Self>) {
        self.chords = chords;
        if fresh {
            self.cursor = 0;
            self.search.update(cx, |f, cx| f.set("", false, cx));
        }
        cx.notify();
    }

    pub fn set_leader(&mut self, waiting: bool, cx: &mut Context<Self>) {
        self.leader = waiting;
        cx.notify();
    }

    pub fn focus(&self, cx: &gpui::App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        chords::filter(&chords::rows(&self.chords), &self.search.read(cx).text)
    }

    /// The panel closes and the chord runs, once this update is over.
    fn pick(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let run = self.chords[row.chord].run.clone();
        self.host.close_layer(LAYER, cx);
        window.defer(cx, move |window, cx| run(window, cx));
    }

    fn row(&self, i: usize, r: &Row, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let shown = if r.caps.is_empty() {
            format!("LEADER  {}", r.leader.to_uppercase())
        } else {
            r.caps.join(" + ")
        };
        let row = r.clone();
        div()
            .id(("keyrow", i))
            .h(px(size::ROW + 4.))
            .px(px(16.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(selected, |d| d.bg(t.raised))
            .hover(|d| d.bg(t.raised))
            .child(
                div()
                    .w(px(CAPS_W))
                    .flex_none()
                    .text_color(if selected {
                        t.accent
                    } else if r.caps.is_empty() {
                        t.dim
                    } else {
                        t.text
                    })
                    .child(shown),
            )
            .child(
                div()
                    .w(px(ARROW_W))
                    .flex_none()
                    .text_color(t.dim)
                    .child("→"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(if selected { t.accent } else { t.text })
                    .child(r.label.clone()),
            )
            .on_click(cx.listener(move |k, _, window, cx| k.pick(&row, window, cx)))
            .into_any_element()
    }
}

impl Render for KeysPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let rows = self.rows(cx);
        let cursor = self.cursor.min(rows.len().saturating_sub(1));
        let mut list = div()
            .id("keys-list")
            .max_h(px(640.))
            .overflow_y_scroll()
            .py(px(6.))
            .flex()
            .flex_col();
        for (i, r) in rows.iter().enumerate() {
            list = list.child(self.row(i, r, i == cursor, cx));
        }
        if rows.is_empty() {
            list = list.child(
                div()
                    .p(px(16.))
                    .text_color(t.dim)
                    .child("No keybinding matches."),
            );
        }
        let host = self.host.clone();
        div()
            .id("keys-panel")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(150.))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                host.close_layer(LAYER, cx)
            })
            .child(
                kit::panel(&t)
                    .w(px(PANEL_W))
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().p(px(14.)).child(self.search.clone()))
                    .child(kit::rule(&t))
                    .child(list)
                    .child(kit::rule(&t))
                    .child(
                        div()
                            .px(px(16.))
                            .py(px(8.))
                            .text_size(px(size::SMALL))
                            .text_color(t.dim)
                            .child(if self.leader {
                                "Leader: press a key now".to_string()
                            } else {
                                format!("{} then a key · Esc closes", chords::LEADER_LABEL)
                            }),
                    ),
            )
    }
}
