//! Brain's search rail and reader.
use crate::page::BrainPage;
use crate::tokens::*;
use gpui::{Context, Div, FontWeight, SharedString, Window, div, prelude::*, px};
use parvi_app_kit::Ask;
use parvi_app_kit::theme::{board, size, theme};
use parvi_app_kit::widgets::{
    kit::{self, Kind},
    markdown,
};

impl Render for BrainPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut list = div()
            .id("brain-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if self.state.query.is_empty() {
            list = list.child(self.render_rail_lists(&t, cx));
        }
        for (i, hit) in self.state.shown().iter().enumerate() {
            let path = hit.path.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("brain-result-{i}")))
                    .debug_selector(move || format!("brain-result-{i}"))
                    .px(px(PAD))
                    .py(px(GAP))
                    .cursor_pointer()
                    .on_click(cx.listener(move |page, _, _, cx| page.brain_open(&path, cx)))
                    .child(hit.title.clone())
                    .child(kit::note(&t, hit.snippet.clone(), Some(t.dim))),
            );
        }
        let count = if self.state.query.is_empty() {
            String::new()
        } else {
            self.state.count()
        };
        let rail = div()
            .w(px(board::LIST_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.divider)
            .child(
                div()
                    .h(px(size::TAB_BAR))
                    .px(px(PAD))
                    .flex()
                    .items_center()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Brain"))
                    .child(div().flex_1())
                    .child(kit::note(&t, count, Some(t.dim))),
            )
            .child(div().p(px(GAP)).child(self.field.clone()))
            .child(list)
            .when_some(self.state.search_error.clone(), |d, e| {
                d.child(kit::note(&t, e, Some(t.error)))
            });
        let mut main = div()
            .id("brain-reader")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p(px(READER_PAD))
            .flex()
            .flex_col()
            .gap(px(PAD));
        if let Some(page) = &self.state.page {
            let send = self.state.reference();
            main = main.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(
                        div()
                            .debug_selector(|| "brain-page-title".into())
                            .flex_1()
                            .font_weight(FontWeight::BOLD)
                            .child(page.title.clone()),
                    )
                    .child(kit::button(&t, "brain-send", "Send", Kind::Plain).on_click(
                        cx.listener(move |page, _, _, cx| {
                            let _ = page.host.ask(
                                Ask::Send {
                                    reference: send.clone(),
                                    target: None,
                                },
                                cx,
                            );
                        }),
                    ))
                    .child(
                        kit::button(&t, "brain-close", "Close", Kind::Quiet).on_click(cx.listener(
                            |page, _, _, cx| {
                                page.state.close();
                                cx.notify();
                            },
                        )),
                    ),
            );
            main = main.child(kit::note(&t, page.path.clone(), Some(t.dim)));
            if !page.history.is_empty() {
                main = main.child(
                    kit::button(&t, "brain-history", "History", Kind::Quiet).on_click(cx.listener(
                        |page, _, _, cx| {
                            page.state.history_open = !page.state.history_open;
                            cx.notify();
                        },
                    )),
                );
                if self.state.history_open {
                    let mut history = div().flex().flex_col().gap(px(GAP_TINY));
                    for commit in &page.history {
                        history = history.child(kit::note(
                            &t,
                            format!("{} · {} · {}", commit.time, commit.author, commit.message),
                            Some(t.dim),
                        ));
                    }
                    main = main.child(history);
                }
            }
            if !page.summary.is_empty() {
                main = main.child(kit::note(&t, page.summary.clone(), Some(t.text2)));
            }
            let weak = cx.weak_entity();
            let links = page.links.clone();
            let on_link =
                std::rc::Rc::new(move |target: &str, _: &mut Window, cx: &mut gpui::App| {
                    if let Some(path) = links
                        .iter()
                        .find(|link| link.target == target)
                        .and_then(|link| link.page.as_ref())
                        .map(|page| page.path.clone())
                    {
                        let _ = weak.update(cx, |page, cx| page.brain_open(&path, cx));
                    }
                });
            let look = markdown::Look {
                t: &t,
                refs: false,
                on_link: Some(on_link),
                select: None,
            };
            main = main.child(markdown::render(
                &look,
                "brain-body",
                &markdown::parse(&page.body),
            ));
            main = main.child(links_section(&page.links, cx));
            if !page.backlinks.is_empty() {
                main = main.child(kit::label(&t, "Linked from"));
                for (i, backlink) in page.backlinks.iter().enumerate() {
                    let path = backlink.path.clone();
                    main = main.child(
                        div()
                            .id(SharedString::from(format!("brain-backlink-{i}")))
                            .cursor_pointer()
                            .on_click(cx.listener(move |page, _, _, cx| page.brain_open(&path, cx)))
                            .child(backlink.title.clone()),
                    );
                }
            }
        } else if let Some(error) = &self.state.page_error {
            main = main.child(kit::note(&t, error.clone(), Some(t.error)));
        } else {
            main = main.child(kit::note(&t, "Select a page to read.", Some(t.dim)));
        }
        let mut trail = div()
            .h(px(size::TAB_BAR))
            .flex_none()
            .px(px(PAD))
            .flex()
            .items_center()
            .gap(px(GAP_TINY))
            .border_b_1()
            .border_color(t.divider);
        for (i, (path, title)) in self.state.trail.iter().enumerate() {
            let path = path.clone();
            trail = trail.child(
                kit::button(
                    &t,
                    SharedString::from(format!("brain-trail-{i}")),
                    title.clone(),
                    Kind::Quiet,
                )
                .on_click(cx.listener(move |page, _, _, cx| page.brain_open(&path, cx))),
            );
        }
        let reader = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(trail)
            .child(main);
        div()
            .id("brain")
            .size_full()
            .flex()
            .bg(t.bg)
            .text_color(t.text)
            .child(rail)
            .child(reader)
    }
}

fn links_section(links: &[crate::state::Link], cx: &mut Context<BrainPage>) -> Div {
    let mut out = div().flex().flex_col().gap(px(GAP_TINY));
    for (i, link) in links.iter().enumerate() {
        if let Some(page) = &link.page {
            let path = page.path.clone();
            out = out.child(
                div()
                    .id(SharedString::from(format!("brain-link-{i}")))
                    .cursor_pointer()
                    .on_click(cx.listener(move |page, _, _, cx| page.brain_open(&path, cx)))
                    .child(page.title.clone()),
            );
        }
    }
    out
}
