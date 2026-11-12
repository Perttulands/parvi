//! Drawing the file panes and viewer.
use crate::open::Show;
use crate::{page::FilesPage, store::paths};
use gpui::{
    AnyElement, Context, Div, MouseButton, MouseMoveEvent, Render, ScrollDelta, Window, div, img,
    prelude::*, px,
};
use parvi_app_kit::theme::{
    board::{GAP, LIST_W, PAD, READER_MAX},
    size, theme,
};
use parvi_app_kit::widgets::kit::{self, Kind};

const ROW_INSET: f32 = 6.;
const SIZE_COLUMN: f32 = 70.;
const MODIFIED_COLUMN: f32 = 100.;
const CARD_W: f32 = 336.;
/// A project card's sessions line and documents line.
const CARD_LINE: f32 = 22.;

impl FilesPage {
    fn projects_landing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut body = div()
            .id("files-projects-list")
            .debug_selector(|| "files-projects-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(ROW_INSET));
        let node = self.side.pane.node.clone();
        let mut roots = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(ROW_INSET))
            .child(div().pr(px(ROW_INSET)).child(kit::label(&t, "Browse")));
        for (index, root) in self.side.pane.roots.iter().enumerate() {
            let folder = root.path.clone();
            let node = node.clone();
            roots = roots.child(
                kit::button(&t, ("browse-root", index), root.name.clone(), Kind::Plain)
                    .debug_selector({
                        let id = format!("files-browse-root-{index}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |p, _, w, cx| {
                        p.browse_project(node.clone(), folder.clone(), None, w, cx)
                    })),
            );
        }
        body = body.child(roots);
        for (node, result) in &self.projects {
            for group in &result.groups {
                body = body.child(
                    div()
                        .pt(px(GAP))
                        .child(kit::label(&t, group.parent.clone())),
                );
                let mut grid = div().flex().flex_wrap().gap(px(ROW_INSET));
                for project in &group.projects {
                    let node_id = node.clone();
                    let folder = project.path.clone();
                    let selector = format!("files-project-{}", project.name);
                    let name = kit::button(
                        &t,
                        gpui::SharedString::from(format!("project-{node}-{folder}")),
                        project.name.clone(),
                        Kind::Quiet,
                    )
                    .w_full()
                    .justify_start()
                    .text_color(t.text)
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |p, _, w, cx| {
                        p.browse_project(node_id.clone(), folder.clone(), None, w, cx)
                    }));
                    // Every card has the same three lines: the project, the
                    // sessions working in it, and its documents.
                    let mut working = div()
                        .h(px(CARD_LINE))
                        .px(px(ROW_INSET))
                        .flex()
                        .items_center()
                        .gap(px(ROW_INSET))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(size::SMALL))
                        .text_color(t.text2);
                    if let Some(sessions) = &self.sessions {
                        for session in &sessions.read(cx).sessions {
                            if session.node == *node
                                && session
                                    .cwd
                                    .as_deref()
                                    .is_some_and(|cwd| paths::inside(&project.path, cwd))
                            {
                                let selector = format!(
                                    "files-project-session-{}-{}",
                                    project.name, session.name
                                );
                                working = working.child(
                                    div()
                                        .flex_none()
                                        .debug_selector(move || selector.clone())
                                        .child(format!("● {}", session.name)),
                                );
                            }
                        }
                    }
                    let mut documents = div()
                        .h(px(CARD_LINE))
                        .flex()
                        .items_center()
                        .overflow_hidden();
                    for doc in &project.documents {
                        let node_id = node.clone();
                        let folder = project.path.clone();
                        let name = doc.clone();
                        let selector = format!("files-project-doc-{}-{doc}", project.name);
                        documents = documents.child(
                            kit::button(
                                &t,
                                gpui::SharedString::from(format!("document-{node}-{folder}-{doc}")),
                                doc.clone(),
                                Kind::Quiet,
                            )
                            .flex_none()
                            .text_size(px(size::SMALL))
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
                    let card = div()
                        .w(px(CARD_W))
                        .flex()
                        .flex_col()
                        .border_1()
                        .border_color(t.divider)
                        .rounded(px(size::RADIUS))
                        .p(px(ROW_INSET / 2.))
                        .child(name)
                        .child(working)
                        .child(documents);
                    grid = grid.child(card);
                }
                body = body.child(grid);
            }
        }
        body
    }
    fn pane(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let side = &self.side;
        let path = side.pane.path.clone().unwrap_or_default();
        let header = paths::shown(&side.pane.roots, &path);
        let mut separators = header.rmatch_indices(paths::separator(&path));
        let last = separators.next().map(|(at, _)| at).unwrap_or(0);
        let previous = separators.next().map(|(at, _)| at).unwrap_or(0);
        let path_start = header[..previous].to_owned();
        let path_middle = header[previous..last].to_owned();
        let path_end = header[last..].to_owned();
        let mut list = div()
            .id("files-list")
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
                let selector = format!("find-result-{name}");
                let relative = paths::relative(side.pane.path.as_deref().unwrap_or(""), &path);
                let label = match line {
                    Some(line) => format!("{relative}:{line}"),
                    None => relative.to_owned(),
                };
                let label_selector = format!("find-label-{label}");
                // A long place gives way in its folders: the name and line
                // always show whole.
                let (folders, leaf) = match label.rfind('/') {
                    Some(cut) => (label[..cut].to_owned(), label[cut..].to_owned()),
                    None => (String::new(), label.clone()),
                };
                list = list.child(
                    kit::button(
                        &t,
                        gpui::SharedString::from(format!("find-{path}-{}", line.unwrap_or(0))),
                        "",
                        Kind::Quiet,
                    )
                    .w_full()
                    .justify_start()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .debug_selector(move || selector.clone())
                    .child(div().debug_selector(move || label_selector.clone()))
                    .when(!folders.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_shrink_1()
                                .min_w(px(ROW_INSET * 3.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(folders),
                        )
                    })
                    .child(div().flex_none().child(leaf))
                    // A hit in a file's text shows its line after the place.
                    .children(item.text.as_deref().map(str::trim).map(|text| {
                        div()
                            .min_w_0()
                            .pl(px(ROW_INSET))
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(t.text)
                            .child(text.to_owned())
                    }))
                    .on_mouse_down(MouseButton::Right, {
                        let path = path.clone();
                        cx.listener(move |p, e: &gpui::MouseDownEvent, window, cx| {
                            window.focus(&p.focus, cx);
                            p.find_context_menu(path.clone(), e.position, cx);
                            cx.stop_propagation();
                        })
                    })
                    .on_click(cx.listener(move |p, _, w, cx| p.open_at(path.clone(), line, w, cx))),
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
                        .id(gpui::SharedString::from(format!("file-{full}")))
                        .flex()
                        .items_center()
                        .w_full()
                        .h(px(size::ROW))
                        .px(px(ROW_INSET))
                        .text_size(px(size::TEXT))
                        .text_color(t.text2)
                        .cursor_pointer()
                        .child({
                            let selector = format!("file-name-{label}");
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .pr(px(ROW_INSET))
                                .debug_selector(move || selector.clone())
                                .child(label)
                        })
                        .child({
                            let value = if folder {
                                String::new()
                            } else {
                                crate::store::format::size(entry.size)
                            };
                            let selector = format!("file-size-{name}-{value}");
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
                            let selector = format!("file-modified-{name}-{value}");
                            div()
                                .w(px(MODIFIED_COLUMN))
                                .flex_none()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .debug_selector(move || selector.clone())
                                .child(value)
                        })
                        .when(selected, |row| {
                            let id = format!("selected-{name}");
                            row.bg(t.raised)
                                .child(div().id("selection").debug_selector(move || id.clone()))
                        })
                        .debug_selector({
                            let id = format!("file-row-{name}");
                            move || id.clone()
                        })
                        .when(folder, |row| {
                            let destination = full.clone();
                            row.on_drop(cx.listener(
                                move |p, paths: &gpui::ExternalPaths, _, cx| {
                                    p.upload_paths(destination.clone(), paths.paths(), cx);
                                    cx.stop_propagation();
                                },
                            ))
                        })
                        .on_mouse_down(MouseButton::Right, {
                            let name = name.clone();
                            cx.listener(move |p, e: &gpui::MouseDownEvent, window, cx| {
                                window.focus(&p.focus, cx);
                                p.context_menu(name.clone(), e.position, cx);
                                cx.stop_propagation();
                            })
                        })
                        .on_click(cx.listener(move |p, _, w, cx| {
                            p.side.pane.select(&name);
                            if folder {
                                p.list(full.clone(), None, cx)
                            } else {
                                p.open(full.clone(), w, cx)
                            }
                        })),
                );
            }
        }
        let parent = side.pane.up();
        let finding = side.pane.find.is_some();
        let kept: crate::store::Kept = self.host.setting(cx).unwrap_or_default();
        let pins = kept
            .pins
            .into_iter()
            .enumerate()
            .map(|(n, pin)| {
                let label = pin.path.clone();
                kit::button(&t, ("pin", n), label, Kind::Quiet)
                    .w_full()
                    .justify_start()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .debug_selector({
                        let id = format!("files-pin-{n}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |p, _, w, cx| p.open_pin(pin.clone(), w, cx)))
            })
            .collect::<Vec<_>>();
        let action = |id: &'static str, label: &'static str| {
            kit::button(&t, id, label, Kind::Quiet).debug_selector(move || id.into())
        };
        div()
            .debug_selector(|| "files-pane".into())
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
                        p.upload_paths(folder, paths.paths(), cx);
                        cx.stop_propagation();
                    }
                })
            })
            .child(
                div()
                    .h(px(size::TAB_BAR))
                    .flex_none()
                    .px(px(ROW_INSET))
                    .flex()
                    .items_center()
                    .gap(px(ROW_INSET))
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        kit::button(&t, "files-up", "Up", Kind::Quiet)
                            .debug_selector(|| "files-up".into())
                            .on_click(cx.listener(move |p, _, _, cx| {
                                if let Some(path) = parent.clone() {
                                    p.list(path, None, cx)
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .overflow_hidden()
                            .text_color(t.text2)
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_shrink_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(path_start),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_shrink_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(path_middle),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .max_w_full()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(t.text)
                                    .debug_selector({
                                        let id = format!("files-path-end-{path_end}");
                                        move || id.clone()
                                    })
                                    .child(path_end),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(ROW_INSET))
                    .px(px(ROW_INSET))
                    .pt(px(ROW_INSET))
                    .child(
                        div()
                            .id("files-search")
                            .debug_selector(|| "files-search".into())
                            .flex_1()
                            .min_w_0()
                            .on_key_down(cx.listener(move |page, e: &gpui::KeyDownEvent, w, cx| {
                                page.search_key(e, w, cx)
                            }))
                            .child(side.search.clone()),
                    )
                    .child(
                        kit::button(
                            &t,
                            "search-mode",
                            if side.search_content {
                                "In text"
                            } else {
                                "In names"
                            },
                            Kind::Quiet,
                        )
                        .debug_selector(|| "search-mode".into())
                        .on_click(cx.listener(move |p, _, _, cx| p.toggle_search_content(cx))),
                    ),
            )
            .children(pins)
            .when(!finding, |pane| {
                pane.child(
                    div()
                        .flex()
                        .flex_none()
                        .px(px(ROW_INSET))
                        .pt(px(ROW_INSET))
                        .text_size(px(size::SMALL))
                        .child(
                            kit::button(&t, "sort-name", "Name", Kind::Quiet)
                                .flex_1()
                                .min_w_0()
                                .justify_start()
                                .px_0()
                                .text_size(px(size::SMALL))
                                .text_color(t.dim)
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.sort(crate::store::listing::Column::Name, cx)
                                })),
                        )
                        .child(
                            kit::button(&t, "sort-size", "Size", Kind::Quiet)
                                .w(px(SIZE_COLUMN))
                                .justify_start()
                                .px_0()
                                .text_size(px(size::SMALL))
                                .text_color(t.dim)
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.sort(crate::store::listing::Column::Size, cx)
                                })),
                        )
                        .child(
                            kit::button(&t, "sort-modified", "Modified", Kind::Quiet)
                                .w(px(MODIFIED_COLUMN))
                                .justify_start()
                                .px_0()
                                .text_size(px(size::SMALL))
                                .text_color(t.dim)
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.sort(crate::store::listing::Column::Modified, cx)
                                })),
                        ),
                )
            })
            .child(list)
            .children(side.pane.error.as_ref().map(|error| {
                div()
                    .id("pane-away")
                    .debug_selector(|| "pane-away".into())
                    .px(px(ROW_INSET))
                    .child(kit::note(&t, error.clone(), Some(t.error)))
            }))
            .children(self.naming.map(|naming| {
                div()
                    .id("files-name")
                    .debug_selector(|| "files-name".into())
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(ROW_INSET))
                    .px(px(ROW_INSET))
                    .pt(px(ROW_INSET))
                    .on_key_down(cx.listener(|page, e: &gpui::KeyDownEvent, w, cx| {
                        if e.keystroke.key == "escape" {
                            page.stop_naming(w, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(div().flex_1().min_w_0().child(side.name.clone()))
                    .child(
                        kit::button(&t, "files-name-apply", naming.verb(), Kind::Plain)
                            .debug_selector(|| "files-name-apply".into())
                            .on_click(cx.listener(|p, _, w, cx| p.apply_naming(w, cx))),
                    )
            }))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .px(px(ROW_INSET))
                    .py(px(ROW_INSET))
                    .border_t_1()
                    .border_color(t.divider)
                    .child(action("files-create-file", "New file").on_click(
                        cx.listener(|p, _, w, cx| p.start_naming(crate::page::Naming::File, w, cx)),
                    ))
                    .child(
                        action("files-create-folder", "New folder").on_click(cx.listener(
                            |p, _, w, cx| p.start_naming(crate::page::Naming::Folder, w, cx),
                        )),
                    )
                    .child(action("files-upload", "Upload…").on_click(cx.listener(
                        move |p, _, window, cx| {
                            if let Some(folder) = p.side.pane.path.clone() {
                                p.pick_uploads(folder, window, cx);
                            }
                        },
                    ))),
            )
    }
    fn viewer(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let side = &self.side;
        let Some(o) = &side.opened else {
            return div()
                .flex_1()
                .min_w_0()
                .border_1()
                .border_color(t.divider)
                .p(px(GAP))
                .child(kit::note(
                    &t,
                    "Open a file from the list, or find one by name or by text.",
                    None,
                ));
        };
        let folder = paths::parent(&o.path);
        let shown_folder = paths::shown(&side.pane.roots, &folder);
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(ROW_INSET))
            .h(px(size::TAB_BAR))
            .px(px(GAP))
            .border_b_1()
            .border_color(t.divider)
            .when(self.dock, |bar| {
                bar.child(
                    kit::button(&t, "files-back", "Back", Kind::Quiet)
                        .debug_selector(|| "files-back".into())
                        .on_click(cx.listener(|p, _, _, cx| {
                            p.side.opened = None;
                            p.keep(cx);
                            cx.notify()
                        })),
                )
            })
            .child(
                div()
                    .id("opened-name")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .debug_selector({
                        let id = format!("file-view-{}", o.name);
                        move || id.clone()
                    })
                    .when(!shown_folder.is_empty(), |name| {
                        name.child(
                            div()
                                .min_w_0()
                                .flex_shrink_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(t.dim)
                                .child(format!("{shown_folder}/")),
                        )
                    })
                    .child(div().flex_none().child(o.name.clone())),
            );
        if o.editing {
            let dirty = self.drafts.contains_key(&o.key());
            bar = bar
                .when(dirty || o.saving, |bar| {
                    bar.child(
                        div()
                            .flex_none()
                            .text_size(px(size::SMALL))
                            .text_color(t.dim)
                            .debug_selector(|| "files-unsaved".into())
                            .child(if o.saving { "Saving…" } else { "Unsaved" }),
                    )
                })
                // Save stands out only while there is something to save.
                .child(
                    kit::button(
                        &t,
                        "files-save",
                        "Save",
                        if dirty { Kind::Primary } else { Kind::Quiet },
                    )
                    .debug_selector(|| "files-save".into())
                    .when(!dirty, |b| b.opacity(parvi_app_kit::theme::fade::IDLE))
                    .on_click(cx.listener(move |p, _, _, cx| p.save(cx))),
                )
                .child(
                    kit::button(
                        &t,
                        "files-discard",
                        if dirty { "Discard" } else { "Done" },
                        Kind::Quiet,
                    )
                    .on_click(cx.listener(move |p, _, w, cx| p.discard(w, cx))),
                );
        } else if matches!(o.show, Show::Text { .. }) {
            bar = bar.child(
                kit::button(&t, "files-edit", "Edit", Kind::Quiet)
                    .debug_selector(|| "files-edit".into())
                    .on_click(cx.listener(move |p, _, w, cx| p.edit(w, cx))),
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
                .on_click(cx.listener(move |p, _, _, cx| p.toggle_diff(cx))),
            );
        }
        if o.pretty.is_some() && !o.editing {
            bar = bar.child(
                kit::button(
                    &t,
                    "files-json-format",
                    if o.source { "Format" } else { "As written" },
                    Kind::Quiet,
                )
                .debug_selector(|| "files-json-format".into())
                .on_click(cx.listener(move |p, _, _, cx| {
                    if let Some(opened) = p.side.opened.as_mut() {
                        opened.source = !opened.source;
                        cx.notify();
                    }
                })),
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
                    if let Some(opened) = p.side.opened.as_mut() {
                        opened.source = !opened.source;
                        cx.notify();
                    }
                })),
            );
        }
        if let Some(zoom) = self.zoom_bar(&t, cx) {
            bar = bar.child(zoom);
        }
        bar = bar.child(
            kit::button(&t, "files-send", "Send", Kind::Plain)
                .debug_selector(|| "files-send".into())
                .on_click(cx.listener(move |p, _, _, cx| p.send(cx))),
        );
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
                        .id("file-diff")
                        .size_full()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col();
                    if let Some(error) = &diff.error {
                        view = view.child(kit::note(&t, error.clone(), Some(t.error)));
                    }
                    if diff.diff.trim().is_empty() && diff.error.is_none() {
                        view = view.child(
                            div()
                                .debug_selector(|| "files-diff-empty".into())
                                .child(kit::note(&t, "No changes", None)),
                        );
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
                Show::Picture { image, natural } => self.picture(image, *natural, cx),
                Show::Model(m) => div()
                    .id("model-stage")
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        kit::button(&t, "model-reset", "Reset view", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                if let Some(Show::Model(model)) =
                                    p.side.opened.as_mut().map(|o| &mut o.show)
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
                                p.side.opened.as_mut().map(|o| &mut o.show)
                            {
                                model.last =
                                    Some((f32::from(e.position.x), f32::from(e.position.y)));
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(move |p, e: &MouseMoveEvent, _, cx| {
                        if let Some(Show::Model(model)) =
                            p.side.opened.as_mut().map(|o| &mut o.show)
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
                            p.side.opened.as_mut().map(|o| &mut o.show)
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
                    .on_click(cx.listener(move |p, _, _, cx| p.play(cx)))
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
                                page.follow_link(&node, &folder, link, window, cx)
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
                        .id("files-markdown-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .debug_selector(|| "files-markdown-rendered".into())
                        .child(
                            div()
                                .p(px(GAP))
                                .max_w(px(READER_MAX))
                                .debug_selector(|| "files-markdown-content".into())
                                .child(parvi_app_kit::widgets::markdown::render(
                                    &look,
                                    "file-markdown",
                                    blocks,
                                )),
                        )
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
                    let selector = format!("files-code-top-{}", side.code.read(cx).top_line() + 1);
                    div()
                        .size_full()
                        .debug_selector(move || selector.clone())
                        .child(side.code.clone())
                        .into_any_element()
                }
            }
        };
        div()
            .debug_selector(|| "files-viewer-bounds".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .map(|d| {
                if self.beside {
                    d.border_r_1()
                } else {
                    d.border_1()
                }
            })
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
            .debug_selector(|| "files-page".into())
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
        if let Some(at) = self.menu {
            let pane = &self.side.pane;
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
                let item = p.side.pane.selection().map(|e| (e.name.clone(), e.folder));
                if let (Some((name, folder)), Some(dir)) = (item, p.side.pane.path.clone()) {
                    let path = paths::join(&dir, &name);
                    if folder {
                        p.list(path, None, cx)
                    } else {
                        p.open(path, w, cx)
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
                        .on_click(cx.listener(move |p, _, window, cx| {
                            p.menu = None;
                            let pane = &p.side.pane;
                            let folder = pane
                                .selection()
                                .filter(|entry| entry.folder)
                                .and_then(|_| pane.selected_path())
                                .or_else(|| pane.path.clone());
                            if let Some(folder) = folder {
                                p.pick_uploads(folder, window, cx);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    kit::button(&t, "file-menu-copy-path", "Copy path", Kind::Quiet)
                        .debug_selector(|| "file-menu-copy-path".into())
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            if let Some(path) = p.side.pane.selected_path() {
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
                        let pane = &p.side.pane;
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
                            if let Some(path) = p.side.pane.selected_path() {
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
                            if let Some(path) = p.side.pane.selected_path() {
                                p.list(
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
                            if let Some(path) = p.side.pane.path.clone() {
                                p.list(path, None, cx);
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
                        p.toggle_pin(cx);
                    })),
                )
                .when(is_file, |menu| {
                    menu.child(
                        kit::button(&t, "file-menu-download", "Download", Kind::Quiet)
                            .debug_selector(|| "file-menu-download".into())
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.menu = None;
                                p.download(cx);
                            })),
                    )
                })
                .child(
                    kit::button(&t, "file-menu-send", "Send to Session", Kind::Quiet).on_click(
                        cx.listener(move |p, _, _, cx| {
                            p.menu = None;
                            p.send_selected(cx)
                        }),
                    ),
                )
                .child(
                    kit::button(&t, "file-menu-rename", "Rename", Kind::Quiet)
                        .debug_selector(|| "file-menu-rename".into())
                        .on_click(cx.listener(move |p, _, w, cx| {
                            p.menu = None;
                            p.start_naming(crate::page::Naming::Rename, w, cx);
                        })),
                )
                .child(
                    kit::button(
                        &t,
                        "file-menu-delete",
                        if self.side.pane.armed {
                            "Confirm delete"
                        } else {
                            "Delete"
                        },
                        Kind::Quiet,
                    )
                    .debug_selector(|| "file-menu-delete".into())
                    .on_click(cx.listener(move |p, _, _, cx| {
                        let confirmed = p.side.pane.armed;
                        p.mutate("delete", cx);
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
        if self.conflict {
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
                                p.write(crate::open::Write::Copy, cx)
                            })),
                    )
                    .child(
                        kit::button(&t, "conflict-overwrite", "Overwrite", Kind::Quiet).on_click(
                            cx.listener(move |p, _, _, cx| {
                                p.write(crate::open::Write::Overwrite, cx)
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
        if self.confirm_discard {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(kit::note(&t, "Discard unsaved changes?", Some(t.error)))
                    .child(
                        kit::button(&t, "files-confirm-discard", "Discard", Kind::Primary)
                            .debug_selector(|| "files-confirm-discard".into())
                            .on_click(cx.listener(move |p, _, w, cx| p.discard(w, cx))),
                    )
                    .child(
                        kit::button(&t, "files-keep-editing", "Keep editing", Kind::Quiet)
                            .debug_selector(|| "files-keep-editing".into())
                            .on_click(cx.listener(|p, _, window, cx| p.keep_editing(window, cx))),
                    ),
            );
        }
        if self.beside {
            // Beside the terminals it is a column of the page, edge to edge.
            return page.p_0().gap_0().child(self.viewer(cx));
        }
        if self.dock {
            page = page.w(px(LIST_W)).flex_none();
            return if self.side.opened.is_some() {
                page.child(self.viewer(cx))
            } else {
                page.child(self.pane(cx))
            };
        }
        let head =
            kit::page_bar(&t)
                .child(kit::page_title(&t, "Files"))
                .when(!self.landing, |head| {
                    head.child(
                        kit::button(&t, "files-projects", "Projects", Kind::Quiet)
                            .debug_selector(|| "files-projects".into())
                            .on_click(cx.listener(|p, _, _, cx| p.show_projects(cx))),
                    )
                });
        let page = if self.landing {
            page.child(self.projects_landing(cx))
        } else {
            page.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(GAP))
                    .child(div().w(px(LIST_W)).flex_none().flex().child(self.pane(cx)))
                    .child(self.viewer(cx).debug_selector(|| "files-viewer".into())),
            )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(head)
            .child(page.flex_1().min_h_0())
    }
}
