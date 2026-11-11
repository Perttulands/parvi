//! Brain's search rail and reader.
use crate::page::BrainPage;
use crate::tokens::*;
use gpui::{
    Context, Div, Entity, FontWeight, HighlightStyle, KeyDownEvent, MouseButton, SharedString,
    StyledText, Window, div, prelude::*, px,
};
use parvi_app_kit::Ask;
use parvi_app_kit::theme::{alpha, board, size, theme};
use parvi_app_kit::widgets::{
    kit::{self, Kind},
    markdown, select,
};

pub(crate) struct Dock {
    page: Entity<BrainPage>,
}

impl Dock {
    pub(crate) fn new(page: Entity<BrainPage>, cx: &mut Context<Self>) -> Self {
        cx.observe(&page, |_, _, cx| cx.notify()).detach();
        Self { page }
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(DOCK_WIDTH)).h_full().child(
            self.page
                .update(cx, |page, cx| page.render(window, cx).into_any_element()),
        )
    }
}

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
            let hit_selector = format!("brain-hit-{path}");
            let mut snippet = String::new();
            let mut highlights = Vec::new();
            for (part, marked) in crate::state::snippet_parts(&hit.snippet) {
                let start = snippet.len();
                snippet.push_str(&part);
                if marked {
                    highlights.push((
                        start..snippet.len(),
                        HighlightStyle {
                            color: Some(t.text),
                            background_color: Some(alpha(t.accent, MARK)),
                            ..Default::default()
                        },
                    ));
                }
            }
            let kind = hit.kind.clone();
            let summary = hit.summary.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("brain-result-{i}")))
                    .debug_selector(move || format!("brain-result-{i}"))
                    .border_l(px(board::WALL))
                    .border_color(if i == self.state.selected {
                        t.accent
                    } else {
                        t.surface
                    })
                    .when(i == self.state.selected, |d| {
                        d.bg(alpha(t.accent, SELECTED)).child(
                            div().debug_selector(move || format!("brain-result-selected-{i}")),
                        )
                    })
                    .px(px(PAD))
                    .py(px(GAP))
                    .cursor_pointer()
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.state.selected = i;
                        page.brain_open(&path, cx);
                    }))
                    .child(
                        div()
                            .debug_selector(move || hit_selector.clone())
                            .child(hit.title.clone()),
                    )
                    .when(!kind.is_empty(), |d| {
                        let selector = format!("brain-result-kind-{kind}");
                        d.child(
                            div()
                                .debug_selector(move || selector.clone())
                                .text_color(t.dim)
                                .child(kind),
                        )
                    })
                    .when(!summary.is_empty(), |d| {
                        let selector = format!("brain-result-summary-{summary}");
                        d.child(
                            div()
                                .debug_selector(move || selector.clone())
                                .text_color(t.text2)
                                .child(summary),
                        )
                    })
                    .child(StyledText::new(snippet).with_highlights(highlights)),
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
            .child(
                div()
                    .debug_selector(|| "brain-search".into())
                    .p(px(GAP))
                    .child(self.field.clone()),
            )
            .child(list)
            .when_some(self.state.search_error.clone(), |d, e| {
                d.child(kit::note(&t, e, Some(t.error)))
            });
        let mut main = div()
            .id("brain-reader")
            .track_focus(&self.reader_focus)
            .when(self.state.edit.is_none(), |d| {
                d.on_mouse_down(MouseButton::Left, {
                    let focus = self.reader_focus.clone();
                    move |_, window, cx| window.focus(&focus, cx)
                })
            })
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
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
                    .when(self.state.edit.is_none(), |d| {
                        d.child(
                            kit::button(&t, "brain-edit", "Edit", Kind::Plain)
                                .debug_selector(|| "brain-edit".into())
                                .on_click(
                                    cx.listener(|page, _, window, cx| page.brain_edit(window, cx)),
                                ),
                        )
                    })
                    .when_some(self.state.edit.clone(), |d, edit| {
                        d.child(
                            kit::button(
                                &t,
                                "brain-save",
                                if edit.saving { "Saving…" } else { "Save" },
                                Kind::Primary,
                            )
                            .debug_selector(|| "brain-save".into())
                            .on_click(cx.listener(|page, _, _, cx| page.brain_save(cx))),
                        )
                        .child(
                            kit::button(
                                &t,
                                "brain-discard",
                                if edit.discarding {
                                    "Confirm"
                                } else {
                                    "Discard"
                                },
                                Kind::Plain,
                            )
                            .on_click(
                                cx.listener(|page, _, window, cx| page.brain_discard(window, cx)),
                            ),
                        )
                    })
                    .child(
                        kit::button(&t, "brain-send", "Send", Kind::Plain)
                            .debug_selector(|| "brain-send".into())
                            .on_click(cx.listener(move |page, _, _, cx| {
                                let _ = page.host.ask(
                                    Ask::Send {
                                        reference: send.clone(),
                                        target: None,
                                    },
                                    cx,
                                );
                            })),
                    )
                    .child(
                        kit::button(&t, "brain-close", "Close", Kind::Quiet).on_click(cx.listener(
                            |page, _, _, cx| {
                                page.state.close();
                                cx.notify();
                            },
                        )),
                    ),
            );
            let subject = page.path.split('/').collect::<Vec<_>>();
            let subject = if subject.len() > 2 {
                subject[1]
            } else {
                subject.first().copied().unwrap_or("")
            };
            let subject = subject.replace(['-', '_'], " ");
            let changed = if page.updated.is_empty() {
                "never committed".to_string()
            } else {
                format!(
                    "changed {} by {}",
                    crate::state::when(&page.updated, jiff::Timestamp::now()),
                    page.author
                )
            };
            let meta = if subject.is_empty() {
                changed
            } else {
                format!("{subject} · {changed}")
            };
            let selector = format!("brain-meta-{meta}");
            let page_selector = format!("brain-reading-{}", page.path);
            main = main.child(
                div()
                    .debug_selector(move || page_selector.clone())
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(
                        div()
                            .debug_selector(move || selector.clone())
                            .text_color(t.dim)
                            .child(meta),
                    )
                    .when(!page.history.is_empty(), |d| {
                        d.child(
                            kit::button(&t, "brain-history", "History", Kind::Quiet)
                                .debug_selector(|| "brain-history".into())
                                .on_click(cx.listener(|page, _, _, cx| {
                                    page.state.history_open = !page.state.history_open;
                                    cx.notify();
                                })),
                        )
                    }),
            );
            if let Some(error) = &self.state.page_error {
                main = main.child(kit::note(&t, error.clone(), Some(t.error)));
            }
            if self.state.history_open && !page.history.is_empty() {
                let mut history = div()
                    .debug_selector(|| "brain-history-open".into())
                    .flex()
                    .flex_col()
                    .gap(px(GAP_TINY));
                for commit in &page.history {
                    history = history.child(kit::note(
                        &t,
                        format!("{} · {} · {}", commit.time, commit.author, commit.message),
                        Some(t.dim),
                    ));
                }
                main = main.child(history);
            }
            if !page.summary.is_empty() {
                main = main.child(kit::note(&t, page.summary.clone(), Some(t.text2)));
            }
            let weak = cx.weak_entity();
            let links = page.links.clone();
            let selection = self.select.clone();
            let on_link =
                std::rc::Rc::new(move |target: &str, _: &mut Window, cx: &mut gpui::App| {
                    if !selection.borrow().selection.is_empty() {
                        return;
                    }
                    if target.starts_with("http://")
                        || target.starts_with("https://")
                        || target.starts_with("mailto:")
                    {
                        cx.open_url(target);
                        return;
                    }
                    let (target, anchor) = target
                        .split_once('#')
                        .map_or((target, None), |(target, anchor)| {
                            (target, Some(anchor.to_string()))
                        });
                    if target.is_empty() {
                        if let Some(anchor) = anchor {
                            let _ = weak.update(cx, |page, cx| {
                                *page.anchor.borrow_mut() = Some(anchor);
                                cx.notify();
                            });
                        }
                        return;
                    }
                    if let Some(path) = links
                        .iter()
                        .find(|link| link.target == target)
                        .and_then(|link| link.page.as_ref())
                        .map(|page| page.path.clone())
                    {
                        let _ = weak.update(cx, |page, cx| page.brain_open_at(&path, anchor, cx));
                    }
                });
            let look = markdown::Look {
                t: &t,
                refs: false,
                on_link: Some(on_link),
                select: Some(self.select.clone()),
            };
            if self.state.edit.is_some() {
                main = main.child(self.summary.clone()).child(
                    div()
                        .flex_1()
                        .min_h(px(EDITOR_HEIGHT))
                        .child(self.editor.clone()),
                );
            } else {
                self.select.borrow_mut().begin();
                let text =
                    crate::prose::wiki_links(&crate::prose::without_title(&page.body, &page.title));
                let selection = self.select.clone();
                let blocks = markdown::parse(&text);
                let mut slugs = crate::prose::Slugger::default();
                if text.len() != page.body.len()
                    && crate::prose::without_title(&page.body, &page.title).len() != page.body.len()
                {
                    slugs.slug(&page.title);
                }
                let headings: Vec<(String, usize)> = blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, block)| match block {
                        markdown::Block::Heading(_, heading) => {
                            Some((slugs.slug(&markdown::inline(heading, false).text), i))
                        }
                        _ => None,
                    })
                    .collect();
                let anchor = self.anchor.clone();
                let scroll = self.scroll.clone();
                let body = markdown::render(&look, "brain-body", &blocks)
                    .debug_selector(|| "brain-prose".into())
                    .on_children_prepainted(move |bounds, window, cx| {
                        selection.borrow_mut().laid_out();
                        let Some(want) = anchor.borrow_mut().take() else {
                            return;
                        };
                        let want = crate::prose::slug(&want);
                        let Some((_, index)) = headings.iter().find(|(slug, _)| slug == &want)
                        else {
                            return;
                        };
                        let Some(child) = bounds.get(*index) else {
                            return;
                        };
                        let offset = scroll.offset();
                        scroll.set_offset(gpui::point(
                            offset.x,
                            offset.y + scroll.bounds().top() - child.top(),
                        ));
                        window.defer(cx, |window, _| window.refresh());
                    });
                main = main.child(select::track(body, &self.select, cx.entity_id(), |_, _| {}));
            }
            if !page.related.is_empty() {
                main =
                    main.child(kit::label(&t, "Related").debug_selector(|| "brain-related".into()));
                for (i, related) in page.related.iter().enumerate() {
                    let path = related.path.clone();
                    let selector = format!("brain-related-{}", path);
                    main = main.child(
                        div()
                            .id(SharedString::from(format!("brain-related-{i}")))
                            .debug_selector(move || selector.clone())
                            .cursor_pointer()
                            .on_click(cx.listener(move |page, _, _, cx| page.brain_open(&path, cx)))
                            .child(related.title.clone()),
                    );
                }
            }
            if page.links.iter().any(|link| link.page.is_some()) {
                main = main.child(kit::label(&t, "Links").debug_selector(|| "brain-links".into()));
                main = main.child(links_section(&page.links, cx));
            }
            if !page.backlinks.is_empty() {
                main = main.child(
                    kit::label(&t, "Linked from").debug_selector(|| "brain-backlinks".into()),
                );
                for (i, backlink) in page.backlinks.iter().enumerate() {
                    let path = backlink.path.clone();
                    let selector = format!("brain-backlink-{path}");
                    main = main.child(
                        div()
                            .debug_selector(move || selector.clone())
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
            let selector = format!("brain-trail-{path}");
            trail = trail.child(
                kit::button(
                    &t,
                    SharedString::from(format!("brain-trail-{i}")),
                    title.clone(),
                    Kind::Quiet,
                )
                .debug_selector(move || selector.clone())
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
        let selected = self.select.clone();
        let can_copy = self.state.edit.is_none();
        div()
            .id("brain")
            .on_key_down(cx.listener(move |page, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && event.keystroke.modifiers == gpui::Modifiers::none()
                    && page.field.read(cx).is_focused(window)
                {
                    if !page.field.read(cx).text.is_empty() {
                        page.field.update(cx, |field, cx| field.set("", false, cx));
                        page.state.typed("");
                        cx.notify();
                    } else if page.host.docked(cx) {
                        page.close_dock(window, cx);
                    } else {
                        window.focus(&page.reader_focus, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                if can_copy
                    && !page.field.read(cx).is_focused(window)
                    && event.keystroke.modifiers.control
                    && event.keystroke.key == "c"
                    && let Some(text) = selected.borrow().copied()
                {
                    select::copy(text, cx);
                    cx.stop_propagation();
                }
            }))
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
            let selector = format!("brain-link-{path}");
            out = out.child(
                div()
                    .debug_selector(move || selector.clone())
                    .id(SharedString::from(format!("brain-link-{i}")))
                    .cursor_pointer()
                    .on_click(cx.listener(move |page, _, _, cx| page.brain_open(&path, cx)))
                    .child(page.title.clone()),
            );
        }
    }
    out
}
