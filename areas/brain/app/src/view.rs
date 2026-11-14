//! Brain's list and reader on the page skeleton: the search over what
//! arrived lately or what it found, the page read beside it.
use crate::page::BrainPage;
use crate::state::PageRef;
use crate::tokens::*;
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, HighlightStyle, KeyDownEvent, MouseButton,
    SharedString, StyledText, Window, div, prelude::*, px,
};
use parvi_app_kit::menu::MenuRow;
use parvi_app_kit::theme::{FAMILY, Theme, alpha, size, theme};
use parvi_app_kit::widgets::{
    kit::{self, Kind},
    markdown, pane, select,
};

impl Render for BrainPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let (rows, chosen) = self.list_rows(&t, cx);
        if std::mem::take(&mut self.reveal)
            && let Some(at) = chosen
        {
            self.list_scroll.scroll_to_item(at);
        }
        let body = div()
            .id("brain-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .flex()
            .flex_col()
            .children(rows);
        let shown = if !self.beside {
            let list = pane::list_wide_frame(&t, self.list_head(&t), body);
            pane::page(&t, list, self.reader(&t, window, cx))
        } else if self.state.page.is_some() {
            // The panel is one column: the page read, with Back to the list.
            div()
                .size_full()
                .bg(t.bg)
                .child(self.reader(&t, window, cx))
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(t.surface)
                .child(self.list_head(&t).child(self.close(&t, cx)))
                .child(body)
        };
        let selected = self.select.clone();
        let can_copy = self.state.edit.is_none();
        shown
            .id("brain")
            .debug_selector(|| "brain".into())
            .font_family(FAMILY)
            .text_color(t.text)
            .on_key_down(cx.listener(move |page, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && event.keystroke.modifiers == gpui::Modifiers::none()
                    && page.field.read(cx).is_focused(window)
                {
                    if !page.field.read(cx).text.is_empty() {
                        page.field.update(cx, |field, cx| field.set("", false, cx));
                        page.state.typed("");
                        cx.notify();
                    } else if page.beside {
                        page.close_panel(window, cx);
                    } else {
                        window.focus(&page.reader_focus, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "left"
                    && event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.control
                    && page.state.edit.is_none()
                    && !page.field.read(cx).is_focused(window)
                {
                    page.brain_back(cx);
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
    }
}

impl BrainPage {
    fn list_head(&self, t: &Theme) -> Div {
        let count = (!self.state.query.is_empty()).then(|| {
            let count = self.state.count();
            let selector = format!("brain-count-{count}");
            pane::meta(t, count, None).debug_selector(move || selector)
        });
        pane::head(t)
            .child(
                div()
                    .debug_selector(|| "brain-search".into())
                    .flex_1()
                    .min_w_0()
                    .child(self.field.clone()),
            )
            .children(count)
    }

    /// The list's rows, and which of them is marked: the results while a
    /// query stands, else what arrived lately.
    fn list_rows(&self, t: &Theme, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<usize>) {
        let b = &self.state;
        let mut out: Vec<AnyElement> = Vec::new();
        let chosen = b.chosen();
        let note = |words: String, colour| pane::label(t, words).text_color(colour);
        if !b.query.is_empty() {
            if let Some(e) = &b.search_error {
                out.push(note(e.clone(), t.error).into_any_element());
            } else if b.results.is_empty() && !b.searching {
                out.push(
                    pane::empty(t, format!("Nothing found for “{}”.", b.query), None)
                        .into_any_element(),
                );
            }
            for (i, hit) in b.results.iter().enumerate() {
                out.push(self.result_row(t, i, hit, chosen == Some(i), cx));
            }
            return (out, chosen);
        }
        if let Some(e) = &b.git_error {
            out.push(note(e.clone(), t.error).into_any_element());
        }
        if !b.arrivals_read {
            out.push(pane::empty(t, "Reading the brain…", None).into_any_element());
        } else if b.arrivals.is_empty() && b.git_error.is_none() {
            out.push(pane::empty(t, "Nothing has arrived yet.", None).into_any_element());
        } else if !b.arrivals.is_empty() {
            out.push(pane::label(t, "Arrived lately").into_any_element());
        }
        let first = out.len();
        let now = jiff::Timestamp::now();
        for (j, a) in b.arrivals.iter().enumerate() {
            let path = a.page.path.clone();
            let title = format!("brain-arrived-{path}");
            let date = format!("brain-arrived-date-{path}");
            out.push(
                pane::row(t, ("brain-arrival", j), chosen == Some(j))
                    .debug_selector(move || format!("brain-arrival-{j}"))
                    .child(pane::cell(a.page.title.clone()).debug_selector(move || title))
                    .child(
                        pane::meta(t, crate::state::when(&a.updated, now), None)
                            .debug_selector(move || date),
                    )
                    .on_click(
                        cx.listener(move |page, _: &ClickEvent, _, cx| page.brain_open(&path, cx)),
                    )
                    .into_any_element(),
            );
        }
        (out, chosen.map(|at| at + first))
    }

    /// A result: its title and kind on one line, the words that matched
    /// under it with the match marked.
    fn result_row(
        &self,
        t: &Theme,
        i: usize,
        hit: &crate::state::Hit,
        chosen: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut snippet = String::new();
        let mut highlights = Vec::new();
        for (part, marked) in crate::state::snippet_parts(&hit.snippet) {
            let start = snippet.len();
            snippet.push_str(&part.replace('\n', " "));
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
        let snippet = snippet.trim_end().to_string();
        let path = hit.path.clone();
        let title = format!("brain-hit-{path}");
        let kind = (!hit.kind.is_empty()).then(|| {
            let selector = format!("brain-result-kind-{}", hit.kind);
            pane::meta(t, hit.kind.clone(), None).debug_selector(move || selector)
        });
        pane::row(t, ("brain-result", i), chosen)
            .debug_selector(move || format!("brain-result-{i}"))
            .flex_col()
            .items_start()
            .py(px(RESULT_PAD_Y))
            .gap(px(SNIPPET_GAP))
            .when(chosen, |d| {
                d.child(div().debug_selector(move || format!("brain-result-selected-{i}")))
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(size::HEAD_GAP))
                    .child(pane::cell(hit.title.clone()).debug_selector(move || title))
                    .children(kind),
            )
            .when(!snippet.is_empty(), |d| {
                d.child(
                    div()
                        .w_full()
                        .font_weight(FontWeight::NORMAL)
                        .text_size(px(size::SMALL))
                        .text_color(t.text2)
                        .line_clamp(2)
                        .text_ellipsis()
                        .child(StyledText::new(snippet).with_highlights(highlights)),
                )
            })
            .on_click(cx.listener(move |page, _: &ClickEvent, _, cx| {
                page.state.selected = i;
                page.brain_open(&path, cx);
            }))
            .into_any_element()
    }

    /// The page read: its head (Back, title, Send, Edit, more), then the
    /// page; or the editor while an edit is in hand.
    fn reader(&mut self, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let beside = self.beside;
        let mut head = pane::head(t);
        // Back along the trail; in the panel, from its first page to the
        // list.
        if (self.state.trail.len() > 1 || self.beside) && self.state.edit.is_none() {
            head = head.child(
                pane::act(t, "brain-back", "Back")
                    .debug_selector(|| "brain-back".into())
                    .tooltip(kit::tip("Alt+Left"))
                    .on_click(cx.listener(|page, _, _, cx| page.brain_back(cx))),
            );
        }
        let Some(page) = self.state.page.clone() else {
            let why = self.state.page_error.clone().unwrap_or_else(|| {
                if self.state.arrivals_read {
                    "Search for a page.".into()
                } else {
                    "Reading the brain…".into()
                }
            });
            let head = head
                .child(div().flex_1())
                .children(beside.then(|| self.close(t, cx)));
            return pane::detail(t, "brain-reader", head, pane::empty(t, why, None).p_0());
        };
        head = head
            .child(pane::title(t, page.title.clone()).debug_selector(|| "brain-page-title".into()));
        match self.state.edit.clone() {
            Some(edit) => {
                head = head
                    .child(
                        kit::button(
                            t,
                            "brain-save",
                            if edit.saving { "Saving…" } else { "Save" },
                            Kind::Primary,
                        )
                        .debug_selector(|| "brain-save".into())
                        .on_click(cx.listener(|page, _, _, cx| page.brain_save(cx))),
                    )
                    .child(
                        pane::act(
                            t,
                            "brain-discard",
                            if edit.discarding {
                                "Confirm"
                            } else {
                                "Discard"
                            },
                        )
                        .on_click(
                            cx.listener(|page, _, window, cx| page.brain_discard(window, cx)),
                        ),
                    );
                let body = div()
                    .size_full()
                    .p(px(size::PAD))
                    .flex()
                    .flex_col()
                    .gap(px(size::HEAD_GAP))
                    .child(self.summary.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(EDITOR_HEIGHT))
                            .child(self.editor.clone()),
                    );
                return pane::detail_frame(head, body);
            }
            None => {
                let more = self.more_menu(&page, cx);
                head = head
                    .child(
                        pane::send(t, "brain-send", &self.host, self.state.reference())
                            .debug_selector(|| "brain-send".into()),
                    )
                    .child(
                        pane::act(t, "brain-edit", "Edit")
                            .debug_selector(|| "brain-edit".into())
                            .on_click(
                                cx.listener(|page, _, window, cx| page.brain_edit(window, cx)),
                            ),
                    )
                    .child(
                        pane::act(t, "brain-more", "…")
                            .debug_selector(|| "brain-more".into())
                            .tooltip(kit::tip("History, copy its path"))
                            .on_click(cx.listener(move |page, e: &ClickEvent, _, cx| {
                                page.host.menu(more.clone(), e.position(), cx)
                            })),
                    )
                    .children(beside.then(|| self.close(t, cx)));
            }
        }
        let body = self.page_body(t, &page, window, cx);
        pane::detail_scrolled(t, "brain-reader", &self.scroll, head, body)
            .track_focus(&self.reader_focus)
            .on_mouse_down(MouseButton::Left, {
                let focus = self.reader_focus.clone();
                move |_, window, cx| window.focus(&focus, cx)
            })
    }

    fn close(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        pane::act(t, "brain-close", "Close")
            .tooltip(kit::tip("Alt+L"))
            .debug_selector(|| "brain-close-beside".into())
            .on_click(cx.listener(|page, _, window, cx| page.close_panel(window, cx)))
            .into_any_element()
    }

    fn more_menu(&self, page: &crate::state::Page, cx: &mut Context<Self>) -> Vec<MenuRow> {
        let mut rows = Vec::new();
        if !page.history.is_empty() {
            let me = cx.weak_entity();
            let words = if self.state.history_open {
                "Hide history"
            } else {
                "History"
            };
            rows.push(MenuRow::new(words, move |_, cx| {
                let _ = me.update(cx, |page, cx| {
                    page.state.history_open = !page.state.history_open;
                    cx.notify();
                });
            }));
        }
        let file = if page.file.is_empty() {
            page.path.clone()
        } else {
            page.file.clone()
        };
        rows.push(MenuRow::new("Copy path", move |_, cx| {
            select::copy(file.clone(), cx)
        }));
        rows
    }

    fn page_body(
        &mut self,
        t: &Theme,
        page: &crate::state::Page,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut content = div().w_full().flex().flex_col().gap(px(GAP));
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
        content = content.child(
            div()
                .debug_selector(move || page_selector)
                .text_size(px(size::SMALL))
                .text_color(t.dim)
                .child(div().debug_selector(move || selector).child(meta)),
        );
        if let Some(error) = &self.state.page_error {
            content = content.child(kit::note(t, error.clone(), Some(t.error)));
        }
        if self.state.history_open && !page.history.is_empty() {
            let mut history = div()
                .debug_selector(|| "brain-history-open".into())
                .flex()
                .flex_col()
                .gap(px(GAP_TINY));
            for commit in &page.history {
                history = history.child(kit::note(
                    t,
                    format!("{} · {} · {}", commit.time, commit.author, commit.message),
                    Some(t.dim),
                ));
            }
            content = content.child(pane::section(t, "History").child(history));
        }
        if !page.summary.is_empty() {
            content = content.child(kit::note(t, page.summary.clone(), Some(t.text2)));
        }
        content = content.child(self.prose(t, page, cx));
        content = content
            .children(self.page_list(t, ["Related", "brain-related"], &page.related, cx))
            .children(self.page_list(t, ["Linked from", "brain-backlink"], &page.backlinks, cx));
        content
    }

    /// Other pages under a label, each a row that follows to it; `name`
    /// names the section and, with each page's path, its rows.
    fn page_list(
        &self,
        t: &Theme,
        [label, name]: [&'static str; 2],
        pages: &[PageRef],
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if pages.is_empty() {
            return None;
        }
        let mut section = pane::section(t, label)
            .pt(px(SECTION_TOP))
            .debug_selector(move || format!("{name}-section"));
        for (i, p) in pages.iter().enumerate() {
            let path = p.path.clone();
            let selector = format!("{name}-{path}");
            section = section.child(
                pane::row(t, SharedString::from(format!("{name}-{i}")), false)
                    .debug_selector(move || selector)
                    .child(pane::cell(p.title.clone()))
                    .on_click(cx.listener(move |page, _: &ClickEvent, _, cx| {
                        page.brain_follow(&path, None, cx)
                    })),
            );
        }
        Some(section)
    }

    /// The page's Markdown: wiki links work, a link to a heading scrolls to
    /// it, and what is selected copies.
    fn prose(
        &mut self,
        t: &Theme,
        page: &crate::state::Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        let links = page.links.clone();
        let selection = self.select.clone();
        let on_link = std::rc::Rc::new(move |target: &str, _: &mut Window, cx: &mut gpui::App| {
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
                let _ = weak.update(cx, |page, cx| page.brain_follow(&path, anchor, cx));
            }
        });
        let look = markdown::Look {
            t,
            refs: false,
            on_link: Some(on_link),
            select: Some(self.select.clone()),
        };
        self.select.borrow_mut().begin();
        let without_title = crate::prose::without_title(&page.body, &page.title);
        let text = crate::prose::wiki_links(&without_title);
        let selection = self.select.clone();
        let blocks = markdown::parse(&text);
        let mut slugs = crate::prose::Slugger::default();
        if without_title.len() != page.body.len() {
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
                let Some((_, index)) = headings.iter().find(|(slug, _)| slug == &want) else {
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
        select::track(body, &self.select, cx.entity_id(), |_, _| {}).into_any_element()
    }
}
