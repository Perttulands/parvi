//! The Brain's rail while nothing is searched: the collections with their
//! counts, one drawn open on its pages, and what arrived lately, newest
//! first (CHROTE `LibraryView`'s Collections and New arrivals). Hiding a
//! collection leaves it out of the search and the arrivals, on this device.

use crate::page::BrainPage;
use crate::state::{self as brain, Listed};
use crate::tokens::*;
use gpui::{ClickEvent, Context, Div, FontWeight, SharedString, div, prelude::*, px};
use parvi_app_kit::theme::{Theme, alpha, board};
use parvi_app_kit::widgets::kit::{self, Kind};

impl BrainPage {
    /// Reads the collections and the arrivals, and the open collection's
    /// pages: on entering the page and after a save.
    pub fn brain_read_rail(&mut self, cx: &mut Context<Self>) {
        self.node_request(
            "",
            "GET",
            "/api/brain/collections",
            None,
            |s, status, body, _| s.state.collections_read(status, &body),
            cx,
        );
        self.node_request(
            "",
            "GET",
            "/api/brain/changes",
            None,
            |s, status, body, _| s.state.changes_read(status, &body),
            cx,
        );
        if let Some(name) = self.state.open_collection.clone() {
            self.brain_read_listing(name, cx);
        }
    }

    fn brain_read_listing(&mut self, name: String, cx: &mut Context<Self>) {
        let path = format!("/api/brain/collection?name={}", brain::encode(&name));
        self.node_request(
            "",
            "GET",
            &path,
            None,
            move |s, status, body, _| s.state.listing_read(&name, status, &body),
            cx,
        );
    }

    fn brain_toggle_collection(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(name) = self.state.toggle_collection(name) {
            self.brain_read_listing(name, cx);
        }
        cx.notify();
    }

    fn brain_toggle_hidden(&mut self, name: &str, cx: &mut Context<Self>) {
        self.state.toggle_hidden(name);
        self.save_hidden(cx);
        cx.notify();
    }

    pub(crate) fn render_rail_lists(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let b = &self.state;
        let now = jiff::Timestamp::now();
        let mut list = div().flex().flex_col();
        list = list.child(section(t, "Collections"));
        if let Some(e) = &b.collections_error {
            list = list.child(pad(kit::note(t, e.clone(), Some(t.error))));
        }
        match &b.collections {
            None if b.collections_error.is_none() => {
                list = list.child(pad(kit::note(t, "Reading the brain…", Some(t.dim))));
            }
            Some(all) if all.is_empty() => {
                list = list.child(pad(kit::note(t, "No collections.", Some(t.dim))));
            }
            _ => {}
        }
        for (i, c) in b.collections.iter().flatten().enumerate() {
            let open = b.open_collection.as_deref() == Some(c.name.as_str());
            let hidden = b.hidden.contains(&c.name);
            list = list.child(self.collection_row(t, i, &c.name, c.pages, open, hidden, cx));
            if open {
                match &b.listing {
                    None => list = list.child(pad(kit::note(t, "Reading…", Some(t.dim)))),
                    Some(pages) if pages.is_empty() => {
                        list = list.child(pad(kit::note(t, "Nothing here.", Some(t.dim))))
                    }
                    Some(pages) => {
                        for (j, p) in pages.iter().enumerate() {
                            let id = format!("brain-shelf-{j}");
                            list = list.child(self.page_row(t, id, p, true, now, cx));
                        }
                    }
                }
            }
        }
        list = list.child(section(t, "New arrivals"));
        if let Some(e) = &b.git_error {
            list = list.child(pad(kit::note(t, e.clone(), Some(t.error))));
        }
        let arrivals = b.arrivals_shown();
        if arrivals.is_empty() && b.git_error.is_none() {
            list = list.child(pad(kit::note(t, "Nothing has arrived yet.", Some(t.dim))));
        }
        for (j, p) in arrivals.into_iter().enumerate() {
            let id = format!("brain-arrival-{j}");
            list = list.child(self.page_row(t, id, p, false, now, cx));
        }
        list
    }

    #[allow(clippy::too_many_arguments)]
    fn collection_row(
        &self,
        t: &Theme,
        i: usize,
        name: &str,
        pages: usize,
        open: bool,
        hidden: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (to_open, to_hide) = (name.to_string(), name.to_string());
        let toggle = if hidden { "Show" } else { "Hide" };
        div()
            .flex()
            .items_center()
            .px(px(PAD))
            .border_l(px(board::WALL))
            .border_color(if open { t.accent } else { t.surface })
            .child(
                div()
                    .id(SharedString::from(format!("brain-collection-{i}")))
                    .debug_selector(move || format!("brain-collection-{i}"))
                    .flex_1()
                    .min_w_0()
                    .py(px(ROW_PAD))
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .cursor_pointer()
                    .when(hidden, |d| d.text_color(t.dim))
                    .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                        s.brain_toggle_collection(&to_open, cx)
                    }))
                    .child(div().text_color(t.dim).child(if open { "▾" } else { "▸" }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(open, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(name.to_string()),
                    )
                    .child(kit::note(t, pages.to_string(), Some(t.dim))),
            )
            .child(
                kit::button(
                    t,
                    SharedString::from(format!("brain-hide-{i}")),
                    toggle,
                    Kind::Quiet,
                )
                .debug_selector(move || format!("brain-hide-{i}"))
                .on_click(
                    cx.listener(move |s, _: &ClickEvent, _, cx| {
                        s.brain_toggle_hidden(&to_hide, cx)
                    }),
                ),
            )
    }

    /// A page as the rail lists it: its title and when it last moved; a
    /// click opens it in the reader.
    fn page_row(
        &self,
        t: &Theme,
        id: String,
        p: &Listed,
        nested: bool,
        now: jiff::Timestamp,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let path = p.page.path.clone();
        let selector = id.clone();
        let page_selector = format!("brain-rail-page-{}", p.page.path);
        let current = self
            .state
            .page
            .as_ref()
            .is_some_and(|open| open.path == p.page.path);
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .flex()
            .items_center()
            .gap(px(GAP))
            .pl(px(if nested { NESTED_INDENT } else { PAD }))
            .pr(px(PAD))
            .py(px(GAP_TINY))
            .border_l(px(board::WALL))
            .border_color(if current { t.accent } else { t.surface })
            .hover(|d| d.bg(alpha(t.text, HOVER)))
            .cursor_pointer()
            .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| s.brain_open(&path, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .debug_selector(move || page_selector.clone())
                    .child(p.page.title.clone()),
            )
            .child(kit::note(t, brain::when(&p.updated, now), Some(t.dim)))
    }
}

fn section(t: &Theme, label: &str) -> Div {
    div()
        .px(px(PAD))
        .pt(px(PAD))
        .pb(px(GAP_TINY))
        .child(kit::label(t, label.to_string()))
}

fn pad(child: Div) -> Div {
    div().px(px(PAD)).py(px(GAP_TINY)).child(child)
}
