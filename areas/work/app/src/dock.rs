//! Work beside any page (Alt+B): the focused session's project's open work
//! as the page's tree, and an item from it read in place, with Send.
use crate::Work;
use crate::store::work::outline;
use crate::tokens::*;
use crate::view::{Place, status_meta};
use gpui::{AnyElement, AppContext, Context, Entity, Render, Window, div, prelude::*, px};
use parvi_app_kit::theme::{FAMILY, size, theme};
use parvi_app_kit::widgets::{kit, pane};

struct Dock {
    work: Entity<Work>,
}
impl Dock {
    fn new(work: Entity<Work>, cx: &mut Context<Self>) -> Self {
        cx.observe(&work, |_, _, cx| cx.notify()).detach();
        Self { work }
    }
}
impl Render for Dock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.work.update(cx, |work, cx| work.render_dock(cx))
    }
}
impl Work {
    pub(crate) fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        if self.host.docked(cx) {
            self.host.close_dock(cx);
        } else if let Some(work) = self.me.upgrade() {
            self.dock_item = None;
            let view = cx.new(|cx| Dock::new(work, cx));
            self.host.show_dock(view.into(), cx);
        }
    }

    fn render_dock(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let item = self
            .dock_item
            .as_ref()
            .and_then(|key| self.board.position(key));
        let inner = match item {
            Some(row) => self.detail(&t, row, Place::Dock, cx),
            None => {
                let project = self.dock_project(cx);
                let label = project
                    .map(|p| self.board.projects[p].label.clone())
                    .unwrap_or_else(|| "Work".into());
                let head = pane::head(&t).child(pane::title(&t, label)).child(
                    pane::act(&t, "wc-close", "Close")
                        .tooltip(kit::tip("Alt+B"))
                        .on_click(cx.listener(|w, _, _, cx| w.toggle_dock(cx))),
                );
                let lines = project
                    .map(|p| outline::tree(&self.board, p))
                    .unwrap_or_default();
                let mut rows = div().flex().flex_col().py(px(size::ROW_INSET));
                if lines.is_empty() {
                    rows = rows.child(pane::empty(&t, self.why_empty(cx), None));
                }
                for (at, line) in lines.into_iter().enumerate() {
                    let r = &self.board.rows[line.row];
                    let selector = format!("work-column-row-{}", r.id);
                    let key = crate::store::work::RowKey::of(r);
                    rows = rows.child(
                        pane::row(&t, ("work-dock-row", at), false)
                            .debug_selector(move || selector)
                            .pl(px(size::ROW_PAD - size::ROW_INSET
                                + line.depth.min(MAX_DEPTH) as f32 * INDENT))
                            .child(pane::cell(r.title.clone()))
                            .children(status_meta(&t, r).map(|(s, c)| pane::meta(&t, s, Some(c))))
                            .on_click(cx.listener(move |w, _, _, cx| {
                                w.dock_item = Some(key.clone());
                                cx.notify();
                            })),
                    );
                }
                div().size_full().flex().flex_col().child(head).child(
                    div()
                        .id("work-dock-rows")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(rows),
                )
            }
        };
        div()
            .id("work-column")
            .debug_selector(|| "work-column".into())
            .w(px(DOCK_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.divider)
            .font_family(FAMILY)
            .text_color(t.text)
            .child(inner)
            .into_any_element()
    }
}
