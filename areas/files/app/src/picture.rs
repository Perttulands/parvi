//! A picture in a pane (CHROTE `FilePanelViewer` and `ImageGlance`): drawn
//! at the one zoom level every picture shares (`store::files::zoom`), whole
//! in its room at fit, with its size in pixels and the level's words.

use crate::open::Show;
use crate::page::FilesPage;
use crate::store::zoom::{self, Px};
use gpui::{AnyElement, App, Context, Div, RenderImage, Window, canvas, div, img, prelude::*, px};
use parvi_app_kit::theme::{Theme, size};
use parvi_app_kit::widgets::kit::{self, Kind};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// The room a side's picture is drawn in, as last laid out.
pub type Room = Rc<Cell<Option<Px>>>;

const ZOOM_GAP: f32 = 2.;
const ZOOM_PAD: f32 = 6.;
const ZOOM_LABEL_WIDTH: f32 = 44.;

impl FilesPage {
    /// How big pictures are drawn, kept in the settings file.
    pub fn zoom(&self, cx: &App) -> zoom::Level {
        self.host
            .setting::<crate::store::Kept>(cx)
            .unwrap_or_default()
            .zoom
    }

    pub fn set_zoom(&mut self, level: zoom::Level, cx: &mut Context<Self>) {
        let mut kept: crate::store::Kept = self.host.setting(cx).unwrap_or_default();
        kept.zoom = zoom::settle(level);
        let _ = self.host.set_setting(&kept, cx);
        cx.notify();
    }

    /// − and +: the next stop from what side `i`'s picture shows now.
    pub fn step_zoom(&mut self, i: usize, up: bool, cx: &mut Context<Self>) {
        let Some(Show::Picture { natural, .. }) = self.sides[i].opened.as_ref().map(|o| &o.show)
        else {
            return;
        };
        let Some(room) = self.sides[i].room.get() else {
            return;
        };
        let next = zoom::step(self.zoom(cx), *natural, room, up);
        self.set_zoom(next, cx);
    }

    /// Frees the pictures no side shows any more; the window keeps a
    /// decoded picture until told.
    pub fn release_pictures(&mut self, window: &mut Window) {
        let shown: Vec<Arc<RenderImage>> = self
            .sides
            .iter()
            .filter_map(|s| match s.opened.as_ref().map(|o| &o.show) {
                Some(Show::Picture { image, .. }) => Some(image.clone()),
                Some(Show::Model(model)) => Some(model.image.clone()),
                _ => None,
            })
            .collect();
        for gone in self.drawn.drain(..) {
            if !shown.iter().any(|s| Arc::ptr_eq(s, &gone)) {
                let _ = window.drop_image(gone);
            }
        }
        self.drawn = shown;
    }

    /// The picture's words in the file's header: its pixels, Fit, 1:1,
    /// −, the percent it is drawn at, +.
    pub(super) fn zoom_bar(&self, i: usize, t: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Show::Picture { natural, .. }) = self.sides[i].opened.as_ref().map(|o| &o.show)
        else {
            return None;
        };
        let level = self.zoom(cx);
        let percent = self.sides[i]
            .room
            .get()
            .map(|room| format!("{}%", zoom::drawn_percent(*natural, room, level)));
        let word = |id: &'static str, label: &'static str, on: bool| {
            kit::button(
                t,
                (id, i),
                label,
                if on { Kind::Plain } else { Kind::Quiet },
            )
            .debug_selector(move || format!("{id}-{i}"))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(ZOOM_GAP))
                .child(
                    div()
                        .px(px(ZOOM_PAD))
                        .text_size(px(size::SMALL))
                        .text_color(t.text2)
                        .whitespace_nowrap()
                        .child(format!("{} × {}", natural.w, natural.h)),
                )
                .child(
                    word("zoom-fit", "Fit", level.is_none())
                        .on_click(cx.listener(|f, _, _, cx| f.set_zoom(None, cx))),
                )
                .child(
                    word("zoom-1", "1:1", level == Some(100))
                        .on_click(cx.listener(|f, _, _, cx| f.set_zoom(Some(100), cx))),
                )
                .child(
                    word("zoom-out", "−", false)
                        .on_click(cx.listener(move |f, _, _, cx| f.step_zoom(i, false, cx))),
                )
                .child(
                    div()
                        .min_w(px(ZOOM_LABEL_WIDTH))
                        .text_size(px(size::SMALL))
                        .text_color(t.text2)
                        .flex()
                        .justify_center()
                        .children(percent),
                )
                .child(
                    word("zoom-in", "+", false)
                        .on_click(cx.listener(move |f, _, _, cx| f.step_zoom(i, true, cx))),
                ),
        )
    }

    /// The picture at the level: centred while it fits, scrolling inside
    /// the room when a percentage makes it bigger.
    pub(super) fn picture(
        &self,
        i: usize,
        image: &Arc<RenderImage>,
        natural: Px,
        cx: &App,
    ) -> AnyElement {
        let room = self.sides[i].room.clone();
        let mut picture = img(image.clone()).flex_none();
        // The stage is the room, or the picture where it is bigger, so a
        // small picture is centred and a big one scrolls from its top left.
        let mut stage = div().p(px(MARGIN)).flex().items_center().justify_center();
        if let Some(r) = room.get() {
            let d = zoom::draw(natural, r, self.zoom(cx));
            picture = picture.w(px(d.w)).h(px(d.h));
            stage = stage
                .w(px(r.w.max(d.w) + 2. * MARGIN))
                .h(px(r.h.max(d.h) + 2. * MARGIN));
        }
        div()
            .size_full()
            .relative()
            .child(measure(room))
            .child(
                div()
                    .id(("picture", i))
                    .absolute()
                    .inset_0()
                    .overflow_scroll()
                    .child(stage.flex_none().child(picture)),
            )
            .into_any_element()
    }
}

/// Room left around a picture, so it never touches the pane's edges.
const MARGIN: f32 = 16.;

/// Records the room's size as laid out, and draws again when it changed.
fn measure(room: Room) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let now = Px {
                w: (f32::from(bounds.size.width) - 2. * MARGIN).max(1.),
                h: (f32::from(bounds.size.height) - 2. * MARGIN).max(1.),
            };
            if room.get() != Some(now) {
                room.set(Some(now));
                // A refresh asked for while drawing is dropped, so ask after it.
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}
