//! Drawing the Work page on the kit's skeleton (DESIGN.md, Work): the
//! project's open work as a tree in the list, or a search's matches, or the
//! projects to choose from; the item beside it, read as a page reads. The
//! same item view stands beside a terminal and in the dock.

use crate::store::work::{Row, Status, Tracker, prose, when};
use crate::tokens::*;
use crate::{Entry, Work};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent,
    SharedString, Window, div, prelude::*, px,
};
use parvi_app_kit::hub::now_ms;
use parvi_app_kit::theme::{FAMILY, Theme, size, theme};
use parvi_app_kit::widgets::kit;
use parvi_app_kit::widgets::markdown;
use parvi_app_kit::widgets::pane;
use parvi_app_kit::widgets::select;
use std::rc::Rc;

/// Where an item view stands: on the page, beside the page that named it,
/// or in the dock beside any page.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Page,
    Beside,
    Dock,
}

/// A status as the operator needs to see it: under way and blocked are
/// said, ready is not.
pub(crate) fn status_meta(t: &Theme, r: &Row) -> Option<(String, Hsla)> {
    match r.status {
        Status::InProgress => Some(("in progress".into(), t.accent)),
        Status::Blocked => Some(("blocked".into(), t.error)),
        Status::Deferred => Some(("deferred".into(), t.dim)),
        Status::Closed => Some((raw_label(r), t.dim)),
        Status::Open => None,
    }
}

impl Render for Work {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let (rows, chosen) = self.list_rows(&t, cx);
        // The row the keys reached stays in view.
        if std::mem::take(&mut self.reveal)
            && let Some(at) = chosen
        {
            self.list_scroll.scroll_to_item(at);
        }
        let body = div()
            .id("work-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .flex()
            .flex_col()
            .children(rows);
        let list = pane::list_wide_frame(&t, self.list_head(&t, cx), body);
        let detail = match self.shown_row() {
            Some(row) => self.detail(&t, row, Place::Page, cx),
            None => pane::detail(
                &t,
                "work-detail",
                pane::head(&t),
                pane::empty(&t, self.why_empty(cx), None).p_0(),
            ),
        };
        pane::page(&t, list, detail)
            .id("work")
            .key_context("Work")
            .track_focus(&self.focus)
            .font_family(FAMILY)
            .text_color(t.text)
            .on_key_down(cx.listener(|w, e: &gpui::KeyDownEvent, window, cx| {
                if w.focus.is_focused(window) {
                    w.key(e, window, cx);
                } else if !w.alt_key(e, window, cx)
                    && e.keystroke.key == "escape"
                    && w.escape_search(window, cx)
                {
                    cx.stop_propagation();
                }
            }))
    }
}

impl Work {
    /// Why the list holds nothing, in one line.
    pub(crate) fn why_empty(&self, cx: &gpui::App) -> String {
        if self.snapshots.is_empty() {
            return "Reading work…".into();
        }
        if self.board.projects.is_empty() {
            return "No projects are configured on this node: add a `beads` or `github` line to its work settings.".into();
        }
        let Some(p) = self.project(cx) else {
            return "Reading work…".into();
        };
        let info = &self.board.projects[p];
        if !self.query.trim().is_empty() {
            format!("Nothing matches “{}”.", self.query.trim())
        } else if let Some(e) = &info.error {
            format!("{} could not be read: {}", info.label, e.trim())
        } else if !info.read {
            format!("Reading {}…", info.label)
        } else {
            format!("Nothing open in {}.", info.label)
        }
    }

    fn list_head(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let field = div().flex_1().min_w_0().child(self.search.clone());
        if self.picking {
            let act: AnyElement = if self.refreshing.is_empty() {
                pane::act(t, "work-read-again", "Read again")
                    .tooltip(kit::tip("Read every project again now"))
                    .on_click(cx.listener(|w, _, _, cx| w.refresh(cx)))
                    .into_any_element()
            } else {
                pane::meta(t, "Reading…", Some(t.accent))
                    .debug_selector(|| "w-refreshing".into())
                    .into_any_element()
            };
            return pane::head(t).child(field).child(act);
        }
        let label = self
            .project(cx)
            .map(|p| self.board.projects[p].label.clone())
            .unwrap_or_else(|| "Project".into());
        pane::head(t).child(field).child(
            pane::act(t, "work-project", label)
                .debug_selector(|| "work-project".into())
                .max_w(px(PROJECT_ACT_W))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .tooltip(kit::tip("Choose the project (P)"))
                .on_click(cx.listener(|w, _, window, cx| w.start_picking(window, cx))),
        )
    }

    /// The list's rows, and which of them is the one chosen.
    fn list_rows(&self, t: &Theme, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<usize>) {
        if self.picking {
            return self.project_rows(t, cx);
        }
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(p) = self.project(cx) {
            let info = &self.board.projects[p];
            let trouble = info
                .error
                .as_ref()
                .map(|e| format!("Not read: {}", e.trim()))
                .or_else(|| {
                    info.unwatched
                        .as_ref()
                        .map(|w| format!("Not watched: {}", w.trim()))
                });
            if let Some(trouble) = trouble.filter(|_| info.read) {
                out.push(
                    pane::label(t, trouble)
                        .text_color(t.error)
                        .into_any_element(),
                );
            }
        }
        if self.entries.is_empty() {
            out.push(pane::empty(t, self.why_empty(cx), None).into_any_element());
            return (out, None);
        }
        let shown = self.shown_row();
        let mut chosen = None;
        for (at, entry) in self.entries.iter().enumerate() {
            if matches!(entry, Entry::Item { row, .. } if Some(*row) == shown) {
                chosen = Some(out.len());
            }
            out.push(match *entry {
                Entry::Item { row, .. } => self.item_row(t, at, entry, shown == Some(row), cx),
                Entry::Label(text) => pane::label(t, text).into_any_element(),
                Entry::More(n) => {
                    pane::label(t, format!("{n} more: add a word to narrow it")).into_any_element()
                }
            });
        }
        (out, chosen)
    }

    fn item_row(
        &self,
        t: &Theme,
        at: usize,
        entry: &Entry,
        chosen: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Entry::Item { row, depth, away } = *entry else {
            return div().into_any_element();
        };
        let r = &self.board.rows[row];
        let selector = format!("work-row-{}", r.id);
        let status = status_meta(t, r);
        let project = away.then(|| self.board.projects[r.project].label.clone());
        pane::row(t, ("work-row", at), chosen)
            .debug_selector(move || selector)
            .pl(px(
                size::ROW_PAD - size::ROW_INSET + depth.min(MAX_DEPTH) as f32 * INDENT
            ))
            .child(
                pane::cell(r.title.clone())
                    .when(r.status == Status::Closed, |d| d.text_color(t.dim)),
            )
            .children(status.map(|(s, c)| pane::meta(t, s, Some(c))))
            .children(project.map(|p| pane::meta(t, p, None)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |w, _, window, cx| {
                    window.focus(&w.focus, cx);
                    w.select(row, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                    window.focus(&w.focus, cx);
                    w.select(row, cx);
                    w.host.menu(w.item_menu(row), e.position, cx);
                }),
            )
            .into_any_element()
    }

    /// The projects to choose from: most open work first, each with how
    /// much is open, or why it was not read.
    fn project_rows(&self, t: &Theme, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<usize>) {
        let open = crate::store::work::outline::open_counts(&self.board);
        let current = self.project(cx);
        let mut out: Vec<AnyElement> = Vec::new();
        for (at, p) in self.projects_listed(cx).into_iter().enumerate() {
            let info = &self.board.projects[p];
            let (meta, colour) = if info.error.is_some() && !info.read {
                ("not read".to_string(), Some(t.error))
            } else if !info.read {
                ("reading…".to_string(), None)
            } else {
                (open[p].to_string(), None)
            };
            let selector = format!("work-pick-{}", info.label);
            let why = info.error.clone();
            out.push(
                pane::row(t, ("work-pick", at), at == self.picked)
                    .debug_selector(move || selector)
                    .child(
                        pane::cell(info.label.clone())
                            .when(Some(p) == current, |d| d.font_weight(FontWeight::SEMIBOLD)),
                    )
                    .child(pane::meta(t, meta, colour))
                    .when_some(why, |d, why| d.tooltip(kit::tip(why)))
                    .on_click(cx.listener(move |w, _, window, cx| w.choose_project(p, window, cx)))
                    .into_any_element(),
            );
        }
        let picked = Some(self.picked).filter(|&p| p < out.len());
        (out, picked)
    }

    /// The item view: its title and actions on the head line, then what it
    /// is, what it is part of and waits on, its text, and its children.
    pub(crate) fn detail(
        &self,
        t: &Theme,
        row: usize,
        place: Place,
        cx: &mut Context<Self>,
    ) -> Div {
        let r = &self.board.rows[row];
        let mut head = pane::head(t);
        let back = match place {
            Place::Dock => true,
            _ => !self.trail.is_empty(),
        };
        if back {
            head = head.child(
                pane::act(t, "w-back", "Back")
                    .debug_selector(|| "w-back".into())
                    .tooltip(kit::tip("Alt+Left"))
                    .on_click(cx.listener(move |w, _, _, cx| {
                        if place == Place::Dock {
                            w.dock_item = None;
                            cx.notify();
                        } else {
                            w.back(cx);
                        }
                    })),
            );
        }
        let title = r.title.clone();
        head = head
            .child(
                pane::title(t, r.title.clone())
                    .debug_selector(move || format!("work-card-title-{title}")),
            )
            .child(
                pane::send(t, "work-send", &self.host, self.line_of(row))
                    .debug_selector(|| "work-send".into()),
            );
        head = head.child(
            pane::act(t, "work-more", "…")
                .debug_selector(|| "work-more".into())
                .tooltip(kit::tip("Start a session on it, copy its id"))
                .on_click(cx.listener(move |w, e: &ClickEvent, _, cx| {
                    w.host.menu(w.item_menu(row), e.position(), cx)
                })),
        );
        if place != Place::Page {
            head = head.child(
                pane::act(t, "work-close", "Close")
                    .tooltip(kit::tip(if place == Place::Dock {
                        "Alt+B"
                    } else {
                        "Alt+I"
                    }))
                    .on_click(cx.listener(move |w, _, _, cx| {
                        if place == Place::Dock {
                            w.toggle_dock(cx);
                        } else if let Some(from) = w.beside_from {
                            w.host.close_beside(from, cx);
                        }
                    })),
            );
        }
        let body = self.item_body(t, row, place, cx);
        let id = match place {
            Place::Page => "work-detail",
            Place::Beside => "work-beside-detail",
            Place::Dock => "work-dock-detail",
        };
        pane::detail(t, id, head, body)
    }

    fn item_body(&self, t: &Theme, row: usize, place: Place, cx: &mut Context<Self>) -> Div {
        let r = &self.board.rows[row];
        let p = &self.board.projects[r.project];
        let mut facts = vec![r.written_id(&self.board), r.kind.clone()];
        if let Some(pr) = r.priority {
            facts.push(format!("P{pr}"));
        }
        facts.push(p.label.clone());
        if let Some(u) = r.updated {
            facts.push(format!("updated {} ago", when::age(u, now_ms())));
        }
        if !r.assignees.is_empty() {
            facts.push(format!("for {}", r.assignees.join(", ")));
        }
        if !r.labels.is_empty() {
            facts.push(r.labels.join(" "));
        }
        let status = status_meta(t, r);
        let mut body = div().flex().flex_col().gap(px(size::SECTION_GAP)).child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(size::HEAD_GAP))
                .text_size(px(size::SMALL))
                .text_color(t.dim)
                .children(status.map(|(s, c)| div().text_color(c).child(s)))
                .child(facts.join(" · ")),
        );
        if let Some(pf) = &r.pull {
            let mut line = div()
                .flex()
                .flex_wrap()
                .gap(px(size::HEAD_GAP))
                .text_color(t.text2)
                .child(format!("{} → {}", pf.head, pf.base))
                .child(
                    div()
                        .text_color(t.accent)
                        .child(format!("+{}", pf.additions)),
                )
                .child(
                    div()
                        .text_color(t.error)
                        .child(format!("−{}", pf.deletions)),
                );
            if let Some(c) = pf.checks {
                let all = c.passed + c.failed + c.pending;
                line = line.child(
                    div()
                        .text_color(if c.failed > 0 { t.error } else { t.text2 })
                        .child(format!("checks {}/{all}", c.passed)),
                );
            }
            body = body.child(pane::section(t, "Branch").child(line));
        }
        let fam = &r.family;
        let parent: Vec<String> = fam.parent.iter().cloned().collect();
        body = body
            .children(self.family(t, PART_OF, &parent, place, cx))
            .children(self.family(t, "Blocked by", &fam.blockers, place, cx));
        let text = prose::link_ids(r.body.trim(), &|id| {
            self.board.find_from(id, r.project).is_some()
        });
        if !text.is_empty() {
            let blocks = markdown::parse(&text);
            let me = cx.entity().downgrade();
            let project = r.project;
            let on_link: markdown::OnLink = Rc::new(move |link: &str, _, cx| {
                let link = link.to_string();
                let _ = me.update(cx, |w, cx| w.follow_link_in(&link, project, place, cx));
            });
            let look = markdown::Look {
                t,
                refs: p.tracker == Tracker::Github,
                on_link: Some(on_link),
                select: Some(self.prose.clone()),
            };
            let prose_area = div()
                .id("work-card")
                .debug_selector(|| "work-card".into())
                // Ctrl+C reaches the page, which copies the selection.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _, window, cx| {
                        if !w.focus.is_focused(window) {
                            window.focus(&w.focus, cx);
                        }
                    }),
                )
                .child(markdown::render(&look, "work-body", &blocks));
            body = body.child(select::track(
                prose_area,
                &self.prose,
                cx.entity_id(),
                select::copy,
            ));
        } else {
            body = body.child(
                div()
                    .id("work-card")
                    .debug_selector(|| "work-card".into())
                    .text_color(t.dim)
                    .child("No description."),
            );
        }
        let pulls_label = if r.pull.is_some() {
            "Closes"
        } else {
            "Pull requests"
        };
        let pulls = if r.pull.is_some() {
            &fam.blocking
        } else {
            &fam.pulls
        };
        let none = Vec::new();
        let blocking = if r.pull.is_some() {
            &none
        } else {
            &fam.blocking
        };
        body.children(self.family(t, "Children", &fam.children, place, cx))
            .children(self.family(t, "Blocking", blocking, place, cx))
            .children(self.family(t, pulls_label, pulls, place, cx))
    }

    /// A section of the item's family: each a row with its title and its
    /// state, which shows that item; one no store read stays as written.
    fn family(
        &self,
        t: &Theme,
        label: &'static str,
        refs: &[String],
        place: Place,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if refs.is_empty() {
            return None;
        }
        let closed = |found: &Option<usize>| {
            found.is_some_and(|i| self.board.rows[i].status == Status::Closed)
        };
        let mut rows: Vec<(usize, &String, Option<usize>)> = refs
            .iter()
            .enumerate()
            .map(|(k, reference)| (k, reference, self.board.find(reference)))
            .collect();
        // Open work first, as the tree lists it; finished work after, and
        // folded to one line until asked for (the parent always shows).
        rows.sort_by_key(|(k, _, found)| (closed(found), *k));
        let finished = rows.iter().filter(|(_, _, f)| closed(f)).count();
        let fold = finished > 0 && label != PART_OF && !self.show_finished;
        let section = pane::section(t, label);
        let mut list = div().flex().flex_col().mx(px(-(size::ROW_PAD)));
        for (k, reference, found) in rows {
            if fold && closed(&found) {
                continue;
            }
            let row = pane::row(t, SharedString::from(format!("fam-{label}-{k}")), false);
            let row = match found {
                Some(i) => {
                    let r = &self.board.rows[i];
                    let selector = format!("work-family-{label}-{}", r.id);
                    let status = status_meta(t, r);
                    row.debug_selector(move || selector)
                        .child(
                            pane::cell(r.title.clone())
                                .when(r.status == Status::Closed, |d| d.text_color(t.dim)),
                        )
                        .children(status.map(|(s, c)| pane::meta(t, s, Some(c))))
                        .child(pane::meta(t, r.written_id(&self.board), None))
                        .on_click(cx.listener(move |w, _, _, cx| w.go(i, place, cx)))
                }
                None => {
                    let selector = format!("work-family-{label}-{reference}");
                    row.debug_selector(move || selector)
                        .cursor_default()
                        .child(pane::cell(reference.clone()).text_color(t.dim))
                }
            };
            list = list.child(row);
        }
        if fold {
            let selector = format!("work-family-{label}-finished");
            list = list.child(
                pane::row(
                    t,
                    SharedString::from(format!("fam-{label}-finished")),
                    false,
                )
                .debug_selector(move || selector)
                .text_color(t.dim)
                .child(pane::cell(format!("{finished} finished")))
                .on_click(cx.listener(|w, _, _, cx| {
                    w.show_finished = true;
                    cx.notify();
                })),
            );
        }
        Some(section.child(list))
    }

    /// Shows a row reached from the item in `place`: in the dock it
    /// replaces the dock's item, elsewhere it joins the trail.
    pub(crate) fn go(&mut self, row: usize, place: Place, cx: &mut Context<Self>) {
        if place == Place::Dock {
            self.dock_item = self.board.rows.get(row).map(crate::store::work::RowKey::of);
            cx.notify();
        } else {
            self.follow_row(row, cx);
        }
    }

    fn follow_link_in(&mut self, link: &str, project: usize, place: Place, cx: &mut Context<Self>) {
        if place != Place::Dock {
            return self.follow_link(link, project, cx);
        }
        if let Some(id) = link.strip_prefix(prose::BEAD_LINK) {
            if let Some(row) = self.board.find_from(id, project) {
                self.go(row, place, cx);
            }
        } else {
            self.follow_link(link, project, cx);
        }
    }
}

const PART_OF: &str = "Part of";

/// The tracker's own status word, as the operator knows it.
pub(crate) fn raw_label(r: &Row) -> String {
    match (r.status, r.raw_status.as_str()) {
        (Status::Blocked, "open") => "blocked".into(),
        (_, raw) => raw.replace('_', " "),
    }
}
