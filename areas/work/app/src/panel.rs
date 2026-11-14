//! The Work panel (Alt+B), over the terminals as anything beside them is so
//! no session changes size: the focused session's project's open work as
//! the page's tree, or an item from it read in place with Back and Send.
use crate::Work;
use crate::store::work::outline::{self, Line};
use crate::tokens::*;
use crate::view::{Place, status_meta};
use gpui::{
    AnyElement, AppContext, Context, Div, Entity, KeyDownEvent, Render, Window, div, prelude::*, px,
};
use parvi_app_kit::theme::{FAMILY, Theme, size, theme};
use parvi_app_kit::widgets::field::{self, FieldEvent};
use parvi_app_kit::widgets::{kit, pane};

/// The page the Work panel opens beside.
pub(crate) const PANEL_BESIDE: &str = "terminals";

pub(crate) struct Panel {
    work: Entity<Work>,
}
impl Panel {
    fn new(work: Entity<Work>, cx: &mut Context<Self>) -> Self {
        cx.observe(&work, |_, _, cx| cx.notify()).detach();
        Self { work }
    }
}
impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.work.update(cx, |work, cx| work.render_panel(cx))
    }
}
impl Work {
    /// Whether the Work panel is the view beside the terminals.
    pub(crate) fn panel_open(&self, cx: &gpui::App) -> bool {
        self.panel.as_ref().is_some_and(|panel| {
            self.host
                .beside_of(PANEL_BESIDE, cx)
                .is_some_and(|b| b.entity_id() == panel.entity_id())
        })
    }

    /// Opens the Work panel beside the terminals and shows them, its find
    /// holding the keyboard, or closes it.
    pub(crate) fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_open(cx) {
            self.close_panel(cx);
            return;
        }
        let Some(work) = self.me.upgrade() else {
            return;
        };
        let panel = self
            .panel
            .get_or_insert_with(|| cx.new(|cx| Panel::new(work, cx)))
            .clone();
        self.panel_item = None;
        self.panel_picking = false;
        self.panel_project = None;
        self.panel_search.update(cx, |f, cx| f.set("", false, cx));
        self.host.show_beside(PANEL_BESIDE, panel.into(), cx);
        self.host.for_page(PANEL_BESIDE, &[]).show(cx);
        field::focus(&self.panel_search, window, cx);
    }

    /// Typing in the panel's find narrows the tree to the matches; Enter
    /// reads the first.
    pub(crate) fn panel_search_event(
        &mut self,
        ev: FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev == FieldEvent::Submit
            && let Some(&row) = self.panel_rows(cx).first()
        {
            self.panel_item = self.board.rows.get(row).map(crate::store::work::RowKey::of);
            window.focus(&self.panel_focus, cx);
        }
        cx.notify();
    }

    /// The rows the panel lists: what its find matches, here first, or
    /// none while it is empty.
    fn panel_rows(&self, cx: &gpui::App) -> Vec<usize> {
        let found = outline::search(
            &self.board,
            self.panel_project(cx),
            &self.panel_search.read(cx).text,
        );
        found.here.into_iter().chain(found.elsewhere).collect()
    }

    /// Esc in the panel: a find is cleared, else the panel closes.
    fn panel_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.keystroke.key != "escape" || e.keystroke.modifiers.modified() {
            return;
        }
        let search = self.panel_search.read(cx);
        if search.is_focused(window) && !search.text.is_empty() {
            self.panel_search.update(cx, |f, cx| f.set("", false, cx));
        } else {
            self.close_panel(cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn close_panel(&mut self, cx: &mut Context<Self>) {
        if self.panel_open(cx) {
            self.host.close_beside(PANEL_BESIDE, cx);
        }
    }

    fn render_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let item = self
            .panel_item
            .as_ref()
            .and_then(|key| self.board.position(key));
        let inner = match item {
            Some(row) => self.detail(&t, row, Place::Panel, cx),
            None if self.panel_picking => self.panel_projects(&t, cx),
            None => {
                let project = self.panel_project(cx);
                let label = project
                    .map(|p| self.board.projects[p].label.clone())
                    .unwrap_or_else(|| "Project".into());
                let head = pane::head(&t)
                    .child(
                        div().flex_1().min_w_0().flex().child(
                            pane::act(&t, "work-panel-project", label)
                                .debug_selector(|| "work-panel-project".into())
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .tooltip(kit::tip("Choose the project"))
                                .on_click(cx.listener(|w, _, _, cx| {
                                    w.panel_picking = true;
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(self.panel_close(&t, cx));
                let finding = !self.panel_search.read(cx).text.trim().is_empty();
                let lines = if finding {
                    self.panel_rows(cx)
                        .into_iter()
                        .map(|row| Line { row, depth: 0 })
                        .collect()
                } else {
                    project
                        .map(|p| outline::tree(&self.board, p))
                        .unwrap_or_default()
                };
                let mut rows = div().flex().flex_col().py(px(size::ROW_INSET));
                if lines.is_empty() {
                    let why = if finding {
                        "Nothing matches.".into()
                    } else {
                        self.why_empty(cx)
                    };
                    rows = rows.child(pane::empty(&t, why, None));
                }
                for (at, line) in lines.into_iter().enumerate() {
                    let r = &self.board.rows[line.row];
                    let selector = format!("work-panel-row-{}", r.id);
                    let key = crate::store::work::RowKey::of(r);
                    rows = rows.child(
                        pane::row(&t, ("work-panel-row", at), false)
                            .debug_selector(move || selector)
                            .pl(px(size::ROW_PAD - size::ROW_INSET
                                + line.depth.min(MAX_DEPTH) as f32 * INDENT))
                            .child(pane::cell(r.title.clone()))
                            .children(status_meta(&t, r).map(|(s, c)| pane::meta(&t, s, Some(c))))
                            .on_click(cx.listener(move |w, _, _, cx| {
                                w.panel_item = Some(key.clone());
                                cx.notify();
                            })),
                    );
                }
                let find = pane::list_head(
                    &t,
                    Some(
                        div()
                            .debug_selector(|| "work-panel-search".into())
                            .child(self.panel_search.clone())
                            .into_any_element(),
                    ),
                );
                panel_frame(div().flex_none().child(head).child(find), rows)
            }
        };
        // The terminals decide its width.
        div()
            .id("work-panel")
            .debug_selector(|| "work-panel".into())
            .track_focus(&self.panel_focus)
            .on_key_down(cx.listener(Self::panel_key))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .font_family(FAMILY)
            .text_color(t.text)
            .child(inner)
            .into_any_element()
    }

    fn panel_close(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        pane::act(t, "work-close-beside", "Close")
            .debug_selector(|| "work-close-beside".into())
            .tooltip(kit::tip("Alt+B or Esc"))
            .on_click(cx.listener(|w, _, _, cx| w.close_panel(cx)))
    }

    /// The projects to choose from in the panel, as the page lists them:
    /// most open work first, each with how much is open. The one chosen
    /// shows in the panel now and is the page's from then on.
    fn panel_projects(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let open = outline::open_counts(&self.board);
        let current = self.panel_project(cx);
        let head = pane::head(t)
            .child(pane::title(t, "Project"))
            .child(self.panel_close(t, cx));
        let mut rows = div().flex().flex_col().py(px(size::ROW_INSET));
        for (at, p) in self.projects_named("").into_iter().enumerate() {
            let info = &self.board.projects[p];
            let selector = format!("work-panel-pick-{}", info.label);
            rows = rows.child(
                pane::row(t, ("work-panel-pick", at), Some(p) == current)
                    .debug_selector(move || selector)
                    .child(pane::cell(info.label.clone()))
                    .child(pane::meta(t, open[p].to_string(), None))
                    .on_click(cx.listener(move |w, _, _, cx| {
                        w.panel_project = w.board.projects.get(p).map(|p| p.path.clone());
                        w.panel_picking = false;
                        w.keep_project(p, cx);
                    })),
            );
        }
        panel_frame(head, rows)
    }
}

/// The panel's head over its rows, which scroll.
fn panel_frame(head: Div, rows: Div) -> Div {
    div().size_full().flex().flex_col().child(head).child(
        div()
            .id("work-panel-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(rows),
    )
}
