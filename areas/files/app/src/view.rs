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

const ROW_INSET: f32 = 6.;
const SIZE_COLUMN: f32 = 70.;
const MODIFIED_COLUMN: f32 = 100.;

impl FilesPage {
    fn projects_landing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut body = div()
            .id("files-projects-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(GAP));
        for (node, result) in &self.projects {
            for group in &result.groups {
                body = body.child(kit::label(&t, group.parent.clone()));
                for project in &group.projects {
                    let node_id = node.clone();
                    let folder = project.path.clone();
                    let selector = format!("files-project-{}", project.name);
                    let mut card = div()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .border_1()
                        .border_color(t.divider)
                        .p(px(PAD))
                        .child(
                            kit::button(
                                &t,
                                gpui::SharedString::from(format!("project-{node}-{folder}")),
                                project.name.clone(),
                                Kind::Quiet,
                            )
                            .debug_selector(move || selector.clone())
                            .on_click(cx.listener(
                                move |p, _, w, cx| {
                                    p.browse_project(node_id.clone(), folder.clone(), None, w, cx)
                                },
                            )),
                        );
                    if let Some(sessions) = &self.sessions {
                        for session in &sessions.read(cx).sessions {
                            if session.node == *node
                                && session
                                    .cwd
                                    .as_deref()
                                    .is_some_and(|cwd| paths::inside(&project.path, cwd))
                            {
                                let label = format!("Session: {}", session.name);
                                let selector = format!(
                                    "files-project-session-{}-{}",
                                    project.name, session.name
                                );
                                card = card.child(
                                    div()
                                        .debug_selector(move || selector.clone())
                                        .child(kit::note(&t, label, None)),
                                );
                            }
                        }
                    }
                    for doc in &project.documents {
                        let node_id = node.clone();
                        let folder = project.path.clone();
                        let name = doc.clone();
                        let selector = format!("files-project-doc-{}-{doc}", project.name);
                        card = card.child(
                            kit::button(
                                &t,
                                gpui::SharedString::from(format!("document-{node}-{folder}-{doc}")),
                                doc.clone(),
                                Kind::Quiet,
                            )
                            .debug_selector(move || selector.clone())
                            .on_click(cx.listener(
                                move |p, _, w, cx| {
                                    p.browse_project(
                                        node_id.clone(),
                                        folder.clone(),
                                        Some(name.clone()),
                                        w,
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                    body = body.child(card);
                }
            }
        }
        let node = self.sides[0].pane.node.clone();
        for (index, root) in self.sides[0].pane.roots.iter().enumerate() {
            let folder = root.path.clone();
            let node = node.clone();
            body = body.child(
                kit::button(
                    &t,
                    ("browse-root", index),
                    format!("Browse {}", root.name),
                    Kind::Quiet,
                )
                .debug_selector({
                    let id = format!("files-browse-root-{index}");
                    move || id.clone()
                })
                .on_click(cx.listener(move |p, _, w, cx| {
                    p.browse_project(node.clone(), folder.clone(), None, w, cx)
                })),
            );
        }
        body
    }
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
            if found.truncated {
                list = list.child(
                    div()
                        .debug_selector(|| "files-find-truncated".into())
                        .child(kit::note(&t, "Results truncated", None)),
                );
            }
            for item in &found.matches {
                let path = item.path.clone();
                let line = item.line;
                let name = item.name.clone();
                let selector = format!("find-result-{i}-{name}");
                let relative = paths::relative(side.pane.path.as_deref().unwrap_or(""), &path);
                let label = match line {
                    Some(line) => format!("{relative}:{line}"),
                    None => relative.to_owned(),
                };
                let label_selector = format!("find-label-{i}-{label}");
                list = list.child(
                    kit::button(
                        &t,
                        gpui::SharedString::from(format!("find-{i}-{path}-{}", line.unwrap_or(0))),
                        label,
                        Kind::Quiet,
                    )
                    .debug_selector(move || selector.clone())
                    .child(div().debug_selector(move || label_selector.clone()))
                    .on_click(
                        cx.listener(move |p, _, w, cx| p.open_at(i, path.clone(), line, w, cx)),
                    ),
                );
            }
        } else if let Some(find) = &side.pane.find {
            let text = match &find.found {
                None => "Searching…".to_owned(),
                Some(Err(error)) => error.clone(),
                Some(Ok(_)) => unreachable!(),
            };
            list = list.child(kit::note(&t, text, None));
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
                    div()
                        .id(gpui::SharedString::from(format!("file-{i}-{full}")))
                        .flex()
                        .items_center()
                        .w_full()
                        .h(px(size::ROW))
                        .px(px(ROW_INSET))
                        .text_size(px(size::TEXT))
                        .text_color(t.text2)
                        .cursor_pointer()
                        .child({
                            let selector = format!("file-name-{i}-{label}");
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .debug_selector(move || selector.clone())
                                .child(label)
                        })
                        .child({
                            let value = if folder {
                                String::new()
                            } else {
                                crate::store::format::size(entry.size)
                            };
                            let selector = format!("file-size-{i}-{name}-{value}");
                            div()
                                .w(px(SIZE_COLUMN))
                                .flex_none()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .debug_selector(move || selector.clone())
                                .child(value)
                        })
                        .child({
                            let value = crate::store::format::modified(entry.modified);
                            let selector = format!("file-modified-{i}-{name}-{value}");
                            div()
                                .w(px(MODIFIED_COLUMN))
                                .flex_none()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .debug_selector(move || selector.clone())
                                .child(value)
                        })
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
                        .when(folder, |row| {
                            let destination = full.clone();
                            row.on_drop(cx.listener(
                                move |p, paths: &gpui::ExternalPaths, _, cx| {
                                    p.upload_paths(i, destination.clone(), paths.paths(), cx);
                                    cx.stop_propagation();
                                },
                            ))
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
        let kept: crate::store::Kept = self.host.setting(cx).unwrap_or_default();
        let pins = kept
            .pins
            .into_iter()
            .enumerate()
            .map(|(n, pin)| {
                let label = pin.path.clone();
                kit::button(&t, ("pin", i * 1000 + n), label, Kind::Quiet)
                    .debug_selector({
                        let id = format!("files-pin-{i}-{n}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |p, _, w, cx| p.open_pin(i, pin.clone(), w, cx)))
            })
            .collect::<Vec<_>>();
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.divider)
            .on_drop({
                let destination = side.pane.path.clone();
                cx.listener(move |p, paths: &gpui::ExternalPaths, _, cx| {
                    if let Some(folder) = destination.clone() {
                        p.upload_paths(i, folder, paths.paths(), cx);
                        cx.stop_propagation();
                    }
                })
            })
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
                kit::button(&t, ("upload", i), "Upload…", Kind::Plain)
                    .debug_selector({
                        let id = format!("files-upload-{i}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if let Some(folder) = p.sides[i].pane.path.clone() {
                            p.pick_uploads(i, folder, cx);
                        }
                    })),
            )
            .children(pins)
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
                    Kind::Plain,
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
                    .px(px(ROW_INSET))
                    .child(
                        kit::button(&t, ("sort-name", i), "Name", Kind::Quiet)
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .px_0()
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.sort(i, crate::store::listing::Column::Name, cx)
                            })),
                    )
                    .child(
                        kit::button(&t, ("sort-size", i), "Size", Kind::Quiet)
                            .w(px(SIZE_COLUMN))
                            .justify_start()
                            .px_0()
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.sort(i, crate::store::listing::Column::Size, cx)
                            })),
                    )
                    .child(
                        kit::button(&t, ("sort-modified", i), "Modified", Kind::Quiet)
                            .w(px(MODIFIED_COLUMN))
                            .justify_start()
                            .px_0()
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.sort(i, crate::store::listing::Column::Modified, cx)
                            })),
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
                kit::button(&t, ("copy", i), "Copy to other pane", Kind::Plain)
                    .debug_selector({
                        let id = format!("copy-{i}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |p, _, _, cx| p.copy_to_other(i, cx))),
            )
            .child(
                div()
                    .px(px(GAP))
                    .debug_selector({
                        let id = format!("files-name-{i}");
                        move || id.clone()
                    })
                    .child(side.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap(px(GAP))
                    .child(
                        kit::button(&t, ("create-file", i), "New file", Kind::Quiet)
                            .debug_selector({
                                let id = format!("files-create-file-{i}");
                                move || id.clone()
                            })
                            .on_click(cx.listener(move |p, _, _, cx| p.mutate(i, "file", cx))),
                    )
                    .child(
                        kit::button(&t, ("create-folder", i), "New folder", Kind::Quiet)
                            .debug_selector({
                                let id = format!("files-create-folder-{i}");
                                move || id.clone()
                            })
                            .on_click(cx.listener(move |p, _, _, cx| p.mutate(i, "folder", cx))),
                    )
                    .child(
                        kit::button(&t, ("rename", i), "Rename", Kind::Quiet)
                            .debug_selector({
                                let id = format!("files-rename-{i}");
                                move || id.clone()
                            })
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
                        .debug_selector({
                            let id = format!("files-delete-{i}");
                            move || id.clone()
                        })
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
                kit::button(&t, "files-send", "Send", Kind::Quiet)
                    .debug_selector(|| "files-send".into())
                    .on_click(cx.listener(move |p, _, _, cx| p.send(i, cx))),
            )
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
                .debug_selector(|| "files-diff".into())
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
                .debug_selector(|| "files-source".into())
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
                Show::Failed(s) => kit::note(&t, s.clone(), Some(t.error)).into_any_element(),
                Show::Not(s) => {
                    let node = parvi_core::NodeId(o.node.clone());
                    let path = o.path.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .child(kit::note(&t, s.clone(), None))
                        .child(
                            kit::button(&t, "viewer-download", "Download", Kind::Quiet)
                                .debug_selector(|| "viewer-download".into())
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.download_path(node.clone(), path.clone(), cx)
                                })),
                        )
                        .into_any_element()
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
                    div()
                        .size_full()
                        .debug_selector(|| "files-markdown-rendered".into())
                        .child(parvi_app_kit::widgets::markdown::render(
                            &look,
                            "file-markdown",
                            blocks,
                        ))
                        .into_any_element()
                }
                Show::Text { .. }
                    if o.pretty.is_some()
                        && !o.editing
                        && !o.source
                        && !self.drafts.contains_key(&o.key()) =>
                {
                    div()
                        .size_full()
                        .debug_selector(|| "files-json-formatted".into())
                        .child(side.pretty_code.clone())
                        .into_any_element()
                }
                Show::Text { .. } => {
                    let selector =
                        format!("files-code-top-{i}-{}", side.code.read(cx).top_line() + 1);
                    div()
                        .size_full()
                        .debug_selector(move || selector.clone())
                        .child(side.code.clone())
                        .into_any_element()
                }
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
            page = page.child(
                div()
                    .debug_selector({
                        let id = format!("files-error-{e}");
                        move || id.clone()
                    })
                    .child(kit::note(&t, e.clone(), Some(t.error))),
            );
        }
        if let Some(name) = self.upload_conflict_name() {
            page = page.child(
                div()
                    .flex()
                    .gap(px(GAP))
                    .child(kit::note(&t, format!("{name} already exists."), None))
                    .child(
                        kit::button(&t, "upload-replace", "Replace", Kind::Primary)
                            .debug_selector(|| "upload-replace".into())
                            .on_click(cx.listener(|p, _, _, cx| p.resolve_upload(Some(true), cx))),
                    )
                    .child(
                        kit::button(&t, "upload-keep", "Keep both", Kind::Quiet)
                            .debug_selector(|| "upload-keep".into())
                            .on_click(cx.listener(|p, _, _, cx| p.resolve_upload(Some(false), cx))),
                    )
                    .child(
                        kit::button(&t, "upload-cancel", "Cancel", Kind::Quiet)
                            .on_click(cx.listener(|p, _, _, cx| p.resolve_upload(None, cx))),
                    ),
            );
        }
        if let Some(notice) = &self.notice {
            page = page.child(
                div()
                    .debug_selector({
                        let id = format!("files-notice-{notice}");
                        move || id.clone()
                    })
                    .child(kit::note(&t, notice.clone(), None)),
            );
        }
        if let Some((i, at)) = self.menu {
            let pane = &self.sides[i].pane;
            let selected = pane.selected_path();
            let is_file = pane.selection().is_some_and(|e| !e.folder);
            let kept: crate::store::Kept = self.host.setting(cx).unwrap_or_default();
            let pinned = selected.as_ref().is_some_and(|path| {
                kept.pins
                    .iter()
                    .any(|pin| pin.node == pane.node && pin.path == *path)
            });
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
                    kit::button(&t, "file-menu-upload", "Upload…", Kind::Quiet)
                        .debug_selector(|| "file-menu-upload".into())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            let pane = &p.sides[i].pane;
                            let folder = pane
                                .selection()
                                .filter(|entry| entry.folder)
                                .and_then(|_| pane.selected_path())
                                .or_else(|| pane.path.clone());
                            if let Some(folder) = folder {
                                p.pick_uploads(i, folder, cx);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(&t, "file-menu-copy-path", "Copy path", Kind::Quiet)
                        .debug_selector(|| "file-menu-copy-path".into())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            if let Some(path) = p.sides[i].pane.selected_path() {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(path))
                            }
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(
                        &t,
                        "file-menu-copy-relative",
                        "Copy relative path",
                        Kind::Quiet,
                    )
                    .debug_selector(|| "file-menu-copy-relative".into())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        p.menu = None;
                        let pane = &p.sides[i].pane;
                        if let Some(path) = pane.selected_path() {
                            let relative = paths::root_of(&pane.roots, &path)
                                .map(|root| paths::relative(&root.path, &path))
                                .unwrap_or(&path);
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                relative.to_owned(),
                            ));
                        }
                        cx.notify();
                    })),
                )
                .child(
                    kit::button(&t, "file-menu-copy-folder", "Copy folder path", Kind::Quiet)
                        .debug_selector(|| "file-menu-copy-folder".into())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            if let Some(path) = p.sides[i].pane.selected_path() {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    paths::parent(&path),
                                ));
                            }
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(&t, "file-menu-parent", "Open parent", Kind::Quiet)
                        .debug_selector(|| "file-menu-parent".into())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            if let Some(path) = p.sides[i].pane.selected_path() {
                                p.list(
                                    i,
                                    paths::parent(&path),
                                    Some(paths::base_name(&path).to_owned()),
                                    cx,
                                );
                            }
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(&t, "file-menu-refresh", "Refresh", Kind::Quiet)
                        .debug_selector(|| "file-menu-refresh".into())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            if let Some(path) = p.sides[i].pane.path.clone() {
                                p.list(i, path, None, cx);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(
                        &t,
                        "file-menu-pin",
                        if pinned { "Unpin" } else { "Pin" },
                        Kind::Quiet,
                    )
                    .debug_selector(|| "file-menu-pin".into())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        p.menu = None;
                        p.toggle_pin(i, cx);
                    })),
                )
                .when(is_file, |menu| {
                    menu.child(
                        kit::button(&t, "file-menu-download", "Download", Kind::Quiet)
                            .debug_selector(|| "file-menu-download".into())
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.menu = None;
                                p.download(i, cx);
                            })),
                    )
                })
                .child(
                    kit::button(&t, "file-menu-send", "Send to Session", Kind::Quiet).on_click(
                        cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            p.send(i, cx)
                        }),
                    ),
                )
                .child(
                    kit::button(&t, "file-menu-rename", "Rename", Kind::Quiet)
                        .debug_selector(|| "file-menu-rename".into())
                        .on_click(cx.listener(move |p, _, w, cx| {
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
                        })),
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
        if self.dock {
            return page
                .w(px(size::MENU * 2.0))
                .flex_none()
                .child("Files")
                .child(self.pane(0, cx))
                .child(self.viewer(0, cx));
        }
        page = page.child(
            div().flex().gap(px(GAP)).child("FILES").child(
                kit::button(&t, "files-projects", "Projects", Kind::Quiet)
                    .debug_selector(|| "files-projects".into())
                    .on_click(cx.listener(|p, _, _, cx| p.show_projects(cx))),
            ),
        );
        if self.landing {
            return page.child(self.projects_landing(cx));
        }
        page.child(
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
