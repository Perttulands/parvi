//! Drawing the file panes and viewer.
use crate::open::Show;
use crate::{page::FilesPage, store::paths};
use gpui::{
    AnyElement, Context, Div, MouseButton, MouseMoveEvent, Render, ScrollDelta, Window, div, img,
    prelude::*, px,
};
use parvi_app_kit::theme::{
    board::{GAP, PAD},
    size, theme,
};
use parvi_app_kit::widgets::kit::{self, Kind};

impl FilesPage {
    fn pane(&self, i: usize, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let side = &self.sides[i];
        let path = side.pane.path.clone().unwrap_or_default();
        let mut list = div()
            .id(("files-list", i))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(found) = side
            .pane
            .find
            .as_ref()
            .and_then(|f| f.found.as_ref())
            .and_then(|r| r.as_ref().ok())
        {
            for item in &found.matches {
                let path = item.path.clone();
                let line = item.line;
                let name = item.name.clone();
                let selector = format!("find-result-{i}-{name}");
                list = list.child(
                    kit::button(
                        &t,
                        gpui::SharedString::from(format!("find-{i}-{path}")),
                        name,
                        Kind::Quiet,
                    )
                    .debug_selector(move || selector.clone())
                    .on_click(
                        cx.listener(move |p, _, w, cx| p.open_at(i, path.clone(), line, w, cx)),
                    ),
                );
            }
        } else {
            for entry in side.pane.shown() {
                let name = entry.name.clone();
                let label = if entry.folder {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                let full = paths::join(&path, &name);
                let folder = entry.folder;
                let selected = side.pane.selected.as_deref() == Some(name.as_str());
                list = list.child(
                    kit::button(
                        &t,
                        gpui::SharedString::from(format!("file-{i}-{full}")),
                        label,
                        Kind::Quiet,
                    )
                    .when(selected, |row| {
                        let id = format!("selected-{i}-{name}");
                        row.bg(t.raised).child(
                            div()
                                .id(("selection", i))
                                .debug_selector(move || id.clone()),
                        )
                    })
                    .debug_selector({
                        let id = format!("file-row-{i}-{name}");
                        move || id.clone()
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let name = name.clone();
                        cx.listener(move |p, e: &gpui::MouseDownEvent, window, cx| {
                            window.focus(&p.focus, cx);
                            p.context_menu(i, name.clone(), e.position, cx);
                            cx.stop_propagation();
                        })
                    })
                    .on_click(cx.listener(move |p, _, w, cx| {
                        p.active = i;
                        p.sides[i].pane.select(&name);
                        if folder {
                            p.list(i, full.clone(), None, cx)
                        } else {
                            p.open(i, full.clone(), w, cx)
                        }
                    })),
                );
            }
        }
        let parent = side.pane.up();
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.divider)
            .child(
                div()
                    .h(px(size::TAB_BAR))
                    .px(px(GAP))
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(
                        kit::button(&t, ("files-up", i), "Up", Kind::Quiet)
                            .debug_selector({
                                let id = format!("files-up-{i}");
                                move || id.clone()
                            })
                            .on_click(cx.listener(move |p, _, _, cx| {
                                if let Some(path) = parent.clone() {
                                    p.list(i, path, None, cx)
                                }
                            })),
                    )
                    .child(div().overflow_hidden().child(path)),
            )
            .child(
                div()
                    .id(("files-search", i))
                    .debug_selector({
                        let id = format!("files-search-{i}");
                        move || id.clone()
                    })
                    .px(px(GAP))
                    .on_key_down(cx.listener(move |page, e: &gpui::KeyDownEvent, w, cx| {
                        page.search_key(i, e, w, cx)
                    }))
                    .child(side.search.clone()),
            )
            .child(
                kit::button(
                    &t,
                    ("search-mode", i),
                    if side.search_content {
                        "Search contents"
                    } else {
                        "Find names"
                    },
                    Kind::Quiet,
                )
                .debug_selector({
                    let id = format!("search-mode-{i}");
                    move || id.clone()
                })
                .on_click(cx.listener(move |p, _, _, cx| p.toggle_search_content(i, cx))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(GAP))
                    .child(
                        kit::button(&t, ("sort-name", i), "Name", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                p.sort(i, crate::store::listing::Column::Name, cx)
                            }),
                        ),
                    )
                    .child(
                        kit::button(&t, ("sort-size", i), "Size", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                p.sort(i, crate::store::listing::Column::Size, cx)
                            }),
                        ),
                    )
                    .child(
                        kit::button(&t, ("sort-modified", i), "Modified", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                p.sort(i, crate::store::listing::Column::Modified, cx)
                            }),
                        ),
                    ),
            )
            .child(list)
            .children(side.pane.error.as_ref().map(|error| {
                div()
                    .id(("pane-away", i))
                    .debug_selector({
                        let id = format!("pane-away-{i}");
                        move || id.clone()
                    })
                    .child(kit::note(&t, error.clone(), Some(t.error)))
            }))
            .child(
                kit::button(&t, ("copy", i), "Copy to other pane", Kind::Quiet)
                    .debug_selector({
                        let id = format!("copy-{i}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |p, _, _, cx| p.copy_to_other(i, cx))),
            )
            .child(div().px(px(GAP)).child(side.name.clone()))
            .child(
                div()
                    .flex()
                    .gap(px(GAP))
                    .child(
                        kit::button(&t, ("create-file", i), "New file", Kind::Quiet)
                            .on_click(cx.listener(move |p, _, _, cx| p.mutate(i, "file", cx))),
                    )
                    .child(
                        kit::button(&t, ("create-folder", i), "New folder", Kind::Quiet)
                            .on_click(cx.listener(move |p, _, _, cx| p.mutate(i, "folder", cx))),
                    )
                    .child(
                        kit::button(&t, ("rename", i), "Rename", Kind::Quiet)
                            .on_click(cx.listener(move |p, _, _, cx| p.mutate(i, "rename", cx))),
                    )
                    .child(
                        kit::button(
                            &t,
                            ("delete", i),
                            if side.pane.armed {
                                "Confirm delete"
                            } else {
                                "Delete"
                            },
                            Kind::Quiet,
                        )
                        .on_click(cx.listener(move |p, _, _, cx| p.mutate(i, "delete", cx))),
                    ),
            )
    }
    fn viewer(&self, i: usize, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let side = &self.sides[i];
        let Some(o) = &side.opened else {
            return div()
                .flex_1()
                .child(kit::note(&t, "Choose a file to open", None));
        };
        let mut bar = div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .h(px(size::TAB_BAR))
            .child(
                div()
                    .id(("opened-name", i))
                    .debug_selector({
                        let id = format!("file-view-{}", o.name);
                        move || id.clone()
                    })
                    .child(o.name.clone()),
            );
        if o.editing {
            bar = bar
                .child(
                    kit::button(&t, "files-save", "Save", Kind::Primary)
                        .debug_selector(|| "files-save".into())
                        .on_click(cx.listener(move |p, _, _, cx| p.save(i, cx))),
                )
                .child(
                    kit::button(&t, "files-discard", "Discard", Kind::Quiet)
                        .on_click(cx.listener(move |p, _, w, cx| p.discard(i, w, cx))),
                );
        } else if matches!(o.show, Show::Text { .. }) {
            bar = bar.child(
                kit::button(&t, "files-edit", "Edit", Kind::Quiet)
                    .debug_selector(|| "files-edit".into())
                    .on_click(cx.listener(move |p, _, w, cx| p.edit(i, w, cx))),
            );
        }
        if matches!(o.show, Show::Text { .. }) && !o.editing {
            bar = bar.child(
                kit::button(
                    &t,
                    "files-diff",
                    if o.diff_open { "View" } else { "Diff" },
                    Kind::Quiet,
                )
                .on_click(cx.listener(move |p, _, _, cx| p.toggle_diff(i, cx))),
            );
        }
        if matches!(o.show, Show::Text { markdown: Some(_) }) && !o.editing {
            bar = bar.child(
                kit::button(
                    &t,
                    "files-source",
                    if o.source { "Rendered" } else { "Source" },
                    Kind::Quiet,
                )
                .on_click(cx.listener(move |p, _, _, cx| {
                    if let Some(opened) = p.sides[i].opened.as_mut() {
                        opened.source = !opened.source;
                        cx.notify();
                    }
                })),
            );
        }
        if let Some(zoom) = self.zoom_bar(i, &t, cx) {
            bar = bar.child(zoom);
        }
        if self.beside {
            bar = bar.child(
                kit::button(&t, "files-close-beside", "Close", Kind::Quiet)
                    .on_click(cx.listener(|p, _, _, cx| p.close_beside(cx))),
            );
        }
        bar = bar.child(
            kit::button(&t, "files-send", "Send to Session", Kind::Quiet)
                .debug_selector(|| "files-send".into())
                .on_click(cx.listener(move |p, _, _, cx| p.send(i, cx))),
        );
        let body: AnyElement = if o.diff_open {
            match &o.diff {
                None => kit::note(&t, "Reading diff…", None).into_any_element(),
                Some(Err(error)) => kit::note(&t, error.clone(), Some(t.error)).into_any_element(),
                Some(Ok(diff)) => {
                    let mut view = div()
                        .id(("file-diff", i))
                        .size_full()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col();
                    if let Some(error) = &diff.error {
                        view = view.child(kit::note(&t, error.clone(), Some(t.error)));
                    }
                    for row in crate::store::diff::rows(&diff.diff) {
                        let (prefix, color) = match row.kind {
                            crate::store::diff::Kind::Add => {
                                ("+", parvi_app_kit::theme::rgb(t.ansi[2]))
                            }
                            crate::store::diff::Kind::Delete => {
                                ("-", parvi_app_kit::theme::rgb(t.ansi[1]))
                            }
                            crate::store::diff::Kind::Hunk => ("", t.accent),
                            crate::store::diff::Kind::Context => (" ", t.text),
                        };
                        view = view.child(
                            div()
                                .text_color(color)
                                .child(format!("{prefix}{}", row.text)),
                        );
                    }
                    if diff.truncated {
                        view = view.child(kit::note(&t, "Diff truncated", None));
                    }
                    view.into_any_element()
                }
            }
        } else {
            match &o.show {
                Show::Reading => kit::note(&t, "Reading…", None).into_any_element(),
                Show::Failed(s) | Show::Not(s) => {
                    kit::note(&t, s.clone(), Some(t.error)).into_any_element()
                }
                Show::Picture { image, natural } => self.picture(i, image, *natural, cx),
                Show::Model(m) => div()
                    .id(("model-stage", i))
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        kit::button(&t, ("model-reset", i), "Reset view", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                if let Some(Show::Model(model)) =
                                    p.sides[i].opened.as_mut().map(|o| &mut o.show)
                                {
                                    model.reset();
                                    cx.notify()
                                }
                            }),
                        ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |p, e: &gpui::MouseDownEvent, _, _| {
                            if let Some(Show::Model(model)) =
                                p.sides[i].opened.as_mut().map(|o| &mut o.show)
                            {
                                model.last =
                                    Some((f32::from(e.position.x), f32::from(e.position.y)));
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(move |p, e: &MouseMoveEvent, _, cx| {
                        if let Some(Show::Model(model)) =
                            p.sides[i].opened.as_mut().map(|o| &mut o.show)
                        {
                            if e.dragging() {
                                let now = (f32::from(e.position.x), f32::from(e.position.y));
                                if let Some(last) = model.last {
                                    model.yaw += (now.0 - last.0) * 0.01;
                                    model.pitch =
                                        (model.pitch + (now.1 - last.1) * 0.01).clamp(-1.5, 1.5);
                                    model.redraw();
                                    cx.notify();
                                }
                                model.last = Some(now);
                            } else {
                                model.last = None;
                            }
                        }
                    }))
                    .on_scroll(cx.listener(move |p, e: &gpui::ScrollEvent, _, cx| {
                        if let Some(Show::Model(model)) =
                            p.sides[i].opened.as_mut().map(|o| &mut o.show)
                        {
                            let y = match e.delta {
                                ScrollDelta::Lines(v) => v.y,
                                ScrollDelta::Pixels(v) => f32::from(v.y) / 20.,
                            };
                            model.zoom = (model.zoom * (y * 0.1).exp()).clamp(0.1, 10.);
                            model.redraw();
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .child(img(m.image.clone()).max_w_full().max_h_full())
                    .into_any_element(),
                Show::Media(_) => kit::button(&t, "files-play", "Open", Kind::Primary)
                    .debug_selector(|| "files-play".into())
                    .on_click(cx.listener(move |p, _, _, cx| p.play(i, cx)))
                    .into_any_element(),
                Show::Text {
                    markdown: Some(blocks),
                } if !o.editing && !o.source => {
                    let node = o.node.clone();
                    let folder = paths::parent(&o.path);
                    let me = cx.entity().downgrade();
                    let on_link = std::rc::Rc::new(
                        move |link: &str, window: &mut Window, cx: &mut gpui::App| {
                            let _ = me.update(cx, |page, cx| {
                                page.follow_link(i, &node, &folder, link, window, cx)
                            });
                        },
                    );
                    let look = parvi_app_kit::widgets::markdown::Look {
                        t: &t,
                        refs: false,
                        on_link: Some(on_link),
                        select: None,
                    };
                    parvi_app_kit::widgets::markdown::render(&look, "file-markdown", blocks)
                        .into_any_element()
                }
                Show::Text { .. } => side.code.clone().into_any_element(),
            }
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.divider)
            .child(bar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
impl Render for FilesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.release_pictures(window);
        let t = theme(cx).clone();
        let mut page = div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .bg(t.bg)
            .text_color(t.text)
            .flex()
            .flex_col()
            .p(px(PAD))
            .gap(px(GAP));
        if let Some(e) = &self.error {
            page = page.child(kit::note(&t, e.clone(), Some(t.error)));
        }
        if let Some((i, at)) = self.menu {
            let open = cx.listener(move |p, _, w, cx| {
                p.menu = None;
                let item = p.sides[i]
                    .pane
                    .selection()
                    .map(|e| (e.name.clone(), e.folder));
                if let (Some((name, folder)), Some(dir)) = (item, p.sides[i].pane.path.clone()) {
                    let path = paths::join(&dir, &name);
                    if folder {
                        p.list(i, path, None, cx)
                    } else {
                        p.open(i, path, w, cx)
                    }
                }
            });
            let content = div()
                .flex()
                .flex_col()
                .bg(t.surface)
                .border_1()
                .border_color(t.divider)
                .w(px(size::MENU))
                .child(
                    kit::button(&t, "file-menu-open", "Open", Kind::Quiet)
                        .debug_selector(|| "file-menu-open".into())
                        .on_click(open),
                )
                .child(
                    kit::button(&t, "file-menu-copy-path", "Copy path", Kind::Quiet).on_click(
                        cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            if let Some(path) = p.sides[i].pane.selected_path() {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(path))
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    kit::button(&t, "file-menu-send", "Send to Session", Kind::Quiet).on_click(
                        cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            p.send(i, cx)
                        }),
                    ),
                )
                .child(
                    kit::button(&t, "file-menu-rename", "Rename", Kind::Quiet).on_click(
                        cx.listener(move |p, _, w, cx| {
                            p.menu = None;
                            if let Some(name) =
                                p.sides[i].pane.selection().map(|entry| entry.name.clone())
                            {
                                p.sides[i]
                                    .name
                                    .update(cx, |field, cx| field.set(&name, true, cx));
                            }
                            parvi_app_kit::widgets::field::focus(&p.sides[i].name, w, cx);
                            cx.notify()
                        }),
                    ),
                )
                .child(
                    kit::button(
                        &t,
                        "file-menu-delete",
                        if self.sides[i].pane.armed {
                            "Confirm delete"
                        } else {
                            "Delete"
                        },
                        Kind::Quiet,
                    )
                    .on_click(cx.listener(move |p, _, _, cx| {
                        let confirmed = p.sides[i].pane.armed;
                        p.mutate(i, "delete", cx);
                        if confirmed {
                            p.menu = None;
                        }
                        cx.notify()
                    })),
                );
            page = page.child(
                gpui::deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|page, _, _, cx| {
                                page.menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|page, _, _, cx| {
                                page.menu = None;
                                cx.notify();
                            }),
                        )
                        .child(
                            gpui::anchored()
                                .position(at)
                                .snap_to_window_with_margin(px(GAP))
                                .child(content.on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })),
                        ),
                )
                .with_priority(20),
            );
        }
        if let Some(i) = self.conflict {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(kit::note(
                        &t,
                        "The file changed since it was read.",
                        Some(t.error),
                    ))
                    .child(
                        kit::button(&t, "conflict-copy", "Save a copy", Kind::Primary)
                            .debug_selector(|| "conflict-copy".into())
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.write(i, crate::open::Write::Copy, cx)
                            })),
                    )
                    .child(
                        kit::button(&t, "conflict-overwrite", "Overwrite", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                p.write(i, crate::open::Write::Overwrite, cx)
                            }),
                        ),
                    )
                    .child(
                        kit::button(&t, "conflict-cancel", "Keep editing", Kind::Quiet)
                            .on_click(cx.listener(|p, _, w, cx| p.cancel_conflict(w, cx))),
                    ),
            );
        }
        if self.confirm_close_beside {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(kit::note(
                        &t,
                        "Discard unsaved changes and close?",
                        Some(t.error),
                    ))
                    .child(
                        kit::button(
                            &t,
                            "files-close-discard",
                            "Discard and close",
                            Kind::Primary,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.discard_and_close_beside(cx))),
                    )
                    .child(
                        kit::button(&t, "files-close-cancel", "Keep editing", Kind::Quiet)
                            .on_click(cx.listener(|p, _, _, cx| p.cancel_close_beside(cx))),
                    ),
            );
        }
        if let Some(i) = self.confirm_discard {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(kit::note(&t, "Discard unsaved changes?", Some(t.error)))
                    .child(
                        kit::button(&t, "files-confirm-discard", "Discard", Kind::Primary)
                            .debug_selector(|| "files-confirm-discard".into())
                            .on_click(cx.listener(move |p, _, w, cx| p.discard(i, w, cx))),
                    )
                    .child(
                        kit::button(&t, "files-keep-editing", "Keep editing", Kind::Quiet)
                            .debug_selector(|| "files-keep-editing".into())
                            .on_click(cx.listener(|p, _, window, cx| p.keep_editing(window, cx))),
                    ),
            );
        }
        for (i, transfer) in self.transfers.iter().enumerate() {
            let text = format!(
                "{}: {:?} · {} / {}",
                transfer.kept.name,
                transfer.kept.state,
                crate::store::format::size(transfer.kept.bytes),
                crate::store::format::size(transfer.kept.total)
            );
            let mut row = div().flex().gap(px(GAP)).child(kit::note(&t, text, None));
            if let Some(error) = &transfer.kept.error {
                row = row.child(kit::note(&t, error.clone(), Some(t.error)));
            }
            if matches!(
                transfer.kept.state,
                crate::store::transfers::State::Failed | crate::store::transfers::State::Waiting
            ) {
                row = row.child(
                    kit::button(&t, ("retry-copy", i), "Retry", Kind::Quiet)
                        .on_click(cx.listener(move |p, _, _, cx| p.retry_copy(i, cx))),
                );
            }
            page = page.child(row);
        }
        if self.beside {
            return page.child(self.viewer(0, cx));
        }
        page.child(div().child("FILES")).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(GAP))
                .child(self.pane(0, cx))
                .child(self.pane(1, cx))
                .child(self.viewer(self.active, cx)),
        )
    }
}
