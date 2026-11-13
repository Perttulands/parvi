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
use parvi_app_kit::widgets::{kit, pane};

/// The panel's layer on the stack.
pub const LAYER: &str = "keys";
/// The panel, and its key columns: the Alt chord, then the key after the
/// leader.
const PANEL_W: f32 = 560.;
const ALT_W: f32 = 120.;
const LEADER_W: f32 = 150.;

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
        let search = cx.new(|cx| TextField::new(cx, "Search shortcuts"));
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
        let alt = r.caps.join("+");
        let leader = match r.leader.as_str() {
            "" => String::new(),
            "tab" => "Tab".into(),
            k => k.to_string(),
        };
        let row = r.clone();
        div()
            .id(("keyrow", i))
            .h(px(size::ROW))
            .mx(px(size::ROW_INSET))
            .px(px(size::ROW_PAD - size::ROW_INSET))
            .flex()
            .items_center()
            .gap(px(size::HEAD_GAP))
            .rounded(px(size::RADIUS))
            .cursor_pointer()
            .text_color(if selected { t.text } else { t.text2 })
            .when(selected, |d| d.bg(t.raised))
            .hover(|d| d.bg(t.accent_soft()))
            .child(pane::cell(r.label.clone()))
            .child(div().w(px(ALT_W)).flex_none().child(alt))
            .child(
                div()
                    .w(px(LEADER_W))
                    .flex_none()
                    .text_color(t.dim)
                    .child(leader),
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
            .py(px(size::ROW_INSET))
            .flex()
            .flex_col();
        for (i, r) in rows.iter().enumerate() {
            list = list.child(self.row(i, r, i == cursor, cx));
        }
        if rows.is_empty() {
            list = list.child(pane::empty(&t, "No shortcut matches.", None));
        }
        // The columns' names: the leader's own keys, written once here.
        let head = div()
            .h(px(size::ROW))
            .px(px(size::ROW_PAD))
            .flex()
            .items_center()
            .gap(px(size::HEAD_GAP))
            .text_size(px(size::SMALL))
            .text_color(t.dim)
            .child(
                div()
                    .flex_1()
                    .child(if self.leader { "Press a key" } else { "" }),
            )
            .child(div().w(px(ALT_W)).flex_none().child("Key"))
            .child(
                div()
                    .w(px(LEADER_W))
                    .flex_none()
                    .child(format!("After {}", chords::LEADER_LABEL)),
            );
        let host = self.host.clone();
        div()
            .id("keys-panel")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(size::TAB_BAR * 2.))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                host.close_layer(LAYER, cx)
            })
            .child(
                kit::panel(&t)
                    .w(px(PANEL_W))
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().p(px(size::HEAD_PAD)).child(self.search.clone()))
                    .child(kit::rule(&t))
                    .child(head)
                    .child(list),
            )
    }
}
