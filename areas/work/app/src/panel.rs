//! The Work panel (Alt+B), over the terminals as anything beside them is so
//! no session changes size: the focused session's project's open work as
//! the page's tree, or an item from it read in place with Back and Send.
use crate::Work;
use crate::store::work::outline;
use crate::tokens::*;
use crate::view::{Place, status_meta};
use gpui::{AnyElement, AppContext, Context, Div, Entity, Render, Window, div, prelude::*, px};
use parvi_app_kit::theme::{FAMILY, Theme, size, theme};
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

    /// Opens the Work panel beside the terminals and shows them, or closes
    /// it.
    pub(crate) fn toggle_panel(&mut self, cx: &mut Context<Self>) {
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
        self.host.show_beside(PANEL_BESIDE, panel.into(), cx);
        self.host.for_page(PANEL_BESIDE, &[]).show(cx);
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
                let lines = project
                    .map(|p| outline::tree(&self.board, p))
                    .unwrap_or_default();
                let mut rows = div().flex().flex_col().py(px(size::ROW_INSET));
                if lines.is_empty() {
                    rows = rows.child(pane::empty(&t, self.why_empty(cx), None));
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
                panel_frame(head, rows)
            }
        };
        // The terminals decide its width.
        div()
            .id("work-panel")
            .debug_selector(|| "work-panel".into())
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
