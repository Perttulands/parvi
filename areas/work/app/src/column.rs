//! The Work column, docked beside any page.
use crate::Work;
use crate::store::work::{Tracker, column};
use crate::tokens::*;
use crate::view::status_colour;
use gpui::{
    AnyElement, AppContext, Context, Div, Entity, FontWeight, Render, SharedString, Window, div,
    prelude::*, px,
};
use parvi_app_kit::theme::{FAMILY, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};

struct Column {
    work: Entity<Work>,
}
impl Column {
    fn new(work: Entity<Work>, cx: &mut Context<Self>) -> Self {
        cx.observe(&work, |_, _, cx| cx.notify()).detach();
        Self { work }
    }
}
impl Render for Column {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.work.update(cx, |work, cx| work.render_column(cx))
    }
}
impl Work {
    pub(crate) fn toggle_column(&mut self, cx: &mut Context<Self>) {
        if self.host.docked(cx) {
            self.host.close_dock(cx);
        } else if let Some(work) = self.me.upgrade() {
            let view = cx.new(|cx| Column::new(work, cx));
            self.host.show_dock(view.into(), cx);
        }
    }

    pub(crate) fn open_from_column(&mut self, row: usize, cx: &mut Context<Self>) {
        self.open(row, cx);
        self.host.show(cx);
    }

    fn column_rows(&self, label: &str, rows: &[usize], cx: &mut Context<Self>) -> Option<Div> {
        if rows.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_XXS))
            .child(kit::label(&t, label));
        for &i in rows {
            let r = &self.board.rows[i];
            col = col.child(
                div()
                    .id(SharedString::from(format!("wc-{}", r.reference)))
                    .debug_selector({
                        let id = r.id.clone();
                        move || format!("work-column-row-{id}")
                    })
                    .px(px(SPACE_MD))
                    .py(px(SPACE_XS))
                    .rounded(px(size::RADIUS))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.raised))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap(px(SPACE_SM))
                            .text_color(t.text2)
                            .child(
                                div()
                                    .text_color(status_colour(&t, r.status))
                                    .child(r.status.glyph()),
                            )
                            .child(r.id.clone()),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(r.title.clone()),
                    )
                    .on_click(cx.listener(move |w, _, _, cx| w.open_from_column(i, cx))),
            );
        }
        Some(col)
    }

    fn render_column(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let moving = column::moving(&self.board);
        let reading = self.snapshots.is_empty()
            || self
                .board
                .projects
                .iter()
                .filter(|p| p.tracker == Tracker::Beads)
                .any(|p| !p.read && p.error.is_none());
        let mut body = div()
            .id("work-column-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(SPACE_XL))
            .flex()
            .flex_col()
            .gap(px(SPACE_2XL));
        if reading {
            body = body.child(kit::note(&t, "Reading Beads…", None));
        } else if moving.groups.is_empty() && moving.failed.is_empty() {
            body = body.child(kit::note(&t, "Nothing is moving.", None));
        }
        for g in &moving.groups {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_MD))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.board.projects[g.project].label.clone()),
                    )
                    .children(self.column_rows("In progress", &g.in_progress, cx))
                    .children(self.column_rows("Ready", &g.ready, cx)),
            );
        }
        if !moving.failed.is_empty() {
            let mut failed = div()
                .flex()
                .flex_col()
                .gap(px(SPACE_XS))
                .child(kit::label(&t, "Unreadable"));
            for &p in &moving.failed {
                let p = &self.board.projects[p];
                failed = failed.child(kit::note(
                    &t,
                    format!("{}: {}", p.label, p.error.clone().unwrap_or_default()),
                    Some(t.error),
                ));
            }
            body = body.child(failed);
        }
        div()
            .id("work-column")
            .w(px(DOCK_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.divider)
            .font_family(FAMILY)
            .child(
                div()
                    .flex_none()
                    .h(px(size::ROW + SPACE_XL))
                    .px(px(SPACE_XL))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_MD))
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Beads"),
                    )
                    .child(kit::caps(&t, &["ALT".into(), "B".into()]))
                    .child(
                        kit::button(&t, "wc-close", "Close", Kind::Quiet)
                            .on_click(cx.listener(|w, _, _, cx| w.toggle_column(cx))),
                    ),
            )
            .child(body)
            .into_any_element()
    }
}
