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
                let name = item.name.clone();
                list = list.child(
                    kit::button(
                        &t,
                        gpui::SharedString::from(format!("find-{i}-{path}")),
                        name,
                        Kind::Quiet,
                    )
                    .on_click(cx.listener(move |p, _, w, cx| p.open(i, path.clone(), w, cx))),
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
                list = list.child(
                    kit::button(
                        &t,
                        gpui::SharedString::from(format!("file-{i}-{full}")),
                        label,
                        Kind::Quiet,
                    )
                    .debug_selector({
                        let id = format!("file-row-{i}-{name}");
                        move || id.clone()
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
                        kit::button(&t, ("files-up", i), "Up", Kind::Quiet).on_click(cx.listener(
                            move |p, _, _, cx| {
                                if let Some(path) = parent.clone() {
                                    p.list(i, path, None, cx)
                                }
                            },
                        )),
                    )
                    .child(div().overflow_hidden().child(path)),
            )
            .child(div().px(px(GAP)).child(side.search.clone()))
            .child(list)
            .child(
                kit::button(&t, ("copy", i), "Copy to other pane", Kind::Quiet)
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
            .child(o.name.clone());
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
        if self.beside {
            bar = bar.child(
                kit::button(&t, "files-close-beside", "Close", Kind::Quiet)
                    .on_click(cx.listener(|p, _, _, cx| p.close_beside(cx))),
            );
        }
        bar = bar.child(
            kit::button(&t, "files-send", "Send to Session", Kind::Quiet)
                .on_click(cx.listener(move |p, _, _, cx| p.send(i, cx))),
        );
        let body: AnyElement = match &o.show {
            Show::Reading => kit::note(&t, "Reading…", None).into_any_element(),
            Show::Failed(s) | Show::Not(s) => {
                kit::note(&t, s.clone(), Some(t.error)).into_any_element()
            }
            Show::Picture { image, .. } => img(image.clone())
                .max_w_full()
                .max_h_full()
                .into_any_element(),
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
                            model.last = Some((f32::from(e.position.x), f32::from(e.position.y)));
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
                .on_click(cx.listener(move |p, _, _, cx| p.play(i, cx)))
                .into_any_element(),
            Show::Text {
                markdown: Some(blocks),
            } if !o.editing && !o.source => {
                let look = parvi_app_kit::widgets::markdown::Look {
                    t: &t,
                    refs: false,
                    on_link: None,
                    select: None,
                };
                parvi_app_kit::widgets::markdown::render(&look, "file-markdown", blocks)
                    .into_any_element()
            }
            Show::Text { .. } => side.code.clone().into_any_element(),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
