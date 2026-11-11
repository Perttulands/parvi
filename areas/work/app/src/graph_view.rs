//! The work graph beside the table (CHROTE's `WorkGraph`): the selected
//! item's family as cards in waves along their blocking edges, each card a
//! way to select its row. The layout is `store::work::graph`.

use crate::Work;
use crate::store::work::Status;
use crate::store::work::graph::{self, NODE_HEIGHT, NODE_WIDTH};
use crate::tokens::*;
use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, PathBuilder, Pixels, SharedString, canvas, div,
    point, prelude::*, px, rgb,
};
use parvi_app_kit::theme::{alpha, size, theme};
use parvi_app_kit::widgets::boundary;

impl Work {
    /// The graph `wide` px across.
    pub(super) fn graph(&self, selected: usize, wide: f32, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let pane = (wide, GRAPH_H - GRAPH_HEADER_H);
        let g = graph::build(&self.board, selected, pane);
        let s = g.scale;
        let rows = &self.board.rows;
        let mut keys: Vec<&str> = g
            .nodes
            .iter()
            .map(|n| self.board.projects[rows[n.row].project].path.as_str())
            .collect();
        keys.push(self.board.projects[rows[g.head].project].path.as_str());
        let shelves = graph::project_colours(&keys, SHELVES.len());
        let shelf = |row: usize| {
            let key = &self.board.projects[rows[row].project].path;
            gpui::Hsla::from(rgb(SHELVES[shelves[key]]))
        };
        let edges: Vec<[(f32, f32); 4]> = g
            .edges
            .iter()
            .map(|e| graph::edge_path(&g.nodes[e.from], &g.nodes[e.to], g.orientation))
            .collect();
        let ink = t.text2;
        let lines = canvas(
            |_, _, _| (),
            boundary::paint(move |b: Bounds<Pixels>, _: (), window, _| {
                let at = |(x, y): (f32, f32)| point(b.origin.x + px(x * s), b.origin.y + px(y * s));
                for [a, c1, c2, z] in &edges {
                    let mut p = PathBuilder::stroke(px(GRAPH_EDGE_W));
                    p.move_to(at(*a));
                    p.cubic_bezier_to(at(*z), at(*c1), at(*c2));
                    if let Ok(path) = p.build() {
                        window.paint_path(path, ink);
                    }
                }
            }),
        )
        .absolute()
        .top_0()
        .left_0()
        .w(px(g.width * s))
        .h(px(g.height * s));
        let mut drawing = div()
            .relative()
            .flex_none()
            .w(px(g.width * s))
            .h(px(g.height * s))
            .child(lines);
        for (k, n) in g.nodes.iter().enumerate() {
            let r = &rows[n.row];
            let picked = n.row == selected;
            let colour = match r.status {
                Status::Blocked => t.error,
                Status::Closed | Status::Deferred => t.dim,
                _ => t.text,
            };
            let row = n.row;
            drawing = drawing.child(
                div()
                    .id(("wg", k))
                    .absolute()
                    .left(px(n.x * s))
                    .top(px(n.y * s))
                    .w(px(NODE_WIDTH * s))
                    .h(px(NODE_HEIGHT * s))
                    .px(px(SPACE_SM))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .rounded(px(size::RADIUS))
                    .border_1()
                    .border_l_4()
                    .border_color(if picked { t.accent } else { t.divider })
                    .bg(if picked { t.accent_light() } else { t.surface })
                    .when(n.outside, |d| d.opacity(GRAPH_OUTSIDE_OPACITY))
                    .cursor_pointer()
                    .hover(|d| d.bg(alpha(t.text, GRAPH_HOVER)))
                    .text_size(px(size::SMALL * s.max(GRAPH_MIN_TEXT_SCALE)))
                    .child(
                        div()
                            .flex()
                            .gap(px(SPACE_XS))
                            .child(div().text_color(colour).child(r.status.glyph()))
                            .child(div().text_color(shelf(row)).child(r.id.clone())),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if r.status == Status::Closed {
                                t.text2
                            } else {
                                t.text
                            })
                            .child(r.title.clone()),
                    )
                    .tooltip(parvi_app_kit::widgets::kit::tip(format!(
                        "{}: {}, {}",
                        r.id,
                        r.title,
                        r.status.label()
                    )))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |w, _, window, cx| {
                            window.focus(&w.focus, cx);
                            w.follow_row(row, cx);
                        }),
                    ),
            );
        }
        let head = &rows[g.head];
        let up = g.head_parent.map(|p| {
            div()
                .id("wg-up")
                .cursor_pointer()
                .text_color(t.text2)
                .hover(|d| d.text_color(t.accent))
                .child(format!("↑ {}", rows[p].id))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |w, _, _, cx| w.follow_row(p, cx)),
                )
        });
        let omitted = (g.omitted > 0).then(|| {
            div()
                .text_color(t.dim)
                .child(format!("{} more not drawn", g.omitted))
        });
        div()
            .flex_none()
            .h(px(GRAPH_H))
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(t.divider)
            .child(
                div()
                    .flex_none()
                    .h(px(GRAPH_HEADER_H))
                    .px(px(SPACE_XL))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_MD))
                    .text_size(px(size::SMALL))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(format!("{} {}", head.id, head.title))),
                    )
                    .children(omitted)
                    .children(up),
            )
            .child(
                div()
                    .id("work-graph")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(drawing),
            )
    }
}
