//! Drawing the Work tab (docs/design/shots/work.png, CHROTE's Beads view):
//! the rail's project and status choices, the search, the table, the card
//! beside it, and a footer that says what was read and what was not.

use crate::Work;
use crate::fit::{self, Side, width};
use crate::tokens::*;

use crate::store::work::{Column, Row, STATUSES, Status, Tracker, prose, size_label, when};
use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, SharedString, Window, div,
    prelude::*, px, uniform_list,
};
use parvi_app_kit::hub::now_ms;
use parvi_app_kit::theme::{FAMILY, Theme, alpha, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::markdown;
use parvi_app_kit::widgets::select;
use std::rc::Rc;

fn right_aligned(c: Column) -> bool {
    matches!(c, Column::Updated | Column::Size | Column::Kids)
}

pub(super) fn status_colour(t: &Theme, s: Status) -> gpui::Hsla {
    match s {
        Status::Open | Status::InProgress => t.accent,
        Status::Blocked => t.error,
        Status::Deferred | Status::Closed => t.dim,
    }
}

fn check(t: &Theme, on: bool) -> Div {
    div()
        .size(px(CHECK_SIZE))
        .flex_none()
        .rounded(px(CHECK_RADIUS))
        .border_1()
        .border_color(if on { t.accent } else { t.text2 })
        .when(on, |d| d.bg(t.accent))
}

impl Work {
    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let needle = self.rail_search.read(cx).text.trim().to_lowercase();
        let mut order: Vec<usize> = (0..self.board.projects.len()).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&self.board.projects[a], &self.board.projects[b]);
            y.total.cmp(&x.total).then(x.label.cmp(&y.label))
        });
        let head = |label: &str, all: bool, id: &'static str, on: Rc<dyn Fn(&mut Work)>| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(kit::label(&t, label))
                .child(
                    div()
                        .id(id)
                        .text_size(px(size::SMALL))
                        .text_color(if all { t.dim } else { t.accent })
                        .when(!all, |d| d.cursor_pointer())
                        .child("All")
                        .on_click(cx.listener(move |w, _, _, cx| {
                            on(w);
                            w.reslice();
                            cx.notify();
                        })),
                )
        };
        let mut projects = div().flex().flex_col();
        for p in order {
            let info = &self.board.projects[p];
            if !needle.is_empty()
                && !info.label.to_lowercase().contains(&needle)
                && !info.path.to_lowercase().contains(&needle)
            {
                continue;
            }
            let kept = self.slice.keeps_project(&info.path);
            let (path, path2) = (info.path.clone(), info.path.clone());
            let trouble = info.error.is_some() || info.unwatched.is_some();
            projects = projects.child(
                div()
                    .id(("wp", p))
                    .h(px(size::ROW))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_MD))
                    .child(
                        div()
                            .id(("wpc", p))
                            .cursor_pointer()
                            .child(check(&t, kept))
                            .on_click(cx.listener(move |w, _, _, cx| {
                                w.slice.toggle_project(&w.board, &path);
                                w.reslice();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("wpn", p))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if kept { t.text } else { t.text2 })
                            .hover(|d| d.text_color(t.text))
                            .child(info.label.clone())
                            .on_click(cx.listener(move |w, _, _, cx| {
                                w.slice.only_project(&path2);
                                w.reslice();
                                cx.notify();
                            })),
                    )
                    .when(trouble, |d| {
                        d.child(
                            div()
                                .text_color(t.error)
                                .text_size(px(size::SMALL))
                                .child("!"),
                        )
                    })
                    .child(div().text_size(px(size::SMALL)).text_color(t.dim).child(
                        if info.read {
                            self.sliced
                                .project_counts
                                .get(p)
                                .copied()
                                .unwrap_or(0)
                                .to_string()
                        } else {
                            "—".into()
                        },
                    )),
            );
        }
        let mut statuses = div().flex().flex_col();
        for s in STATUSES {
            let kept = self.slice.keeps_status(s);
            statuses = statuses.child(
                div()
                    .h(px(size::ROW))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_MD))
                    .child(
                        div()
                            .id(SharedString::from(format!("wsc-{}", s as usize)))
                            .cursor_pointer()
                            .child(check(&t, kept))
                            .on_click(cx.listener(move |w, _, _, cx| {
                                w.slice.toggle_status(s);
                                w.reslice();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .w(px(STATUS_GLYPH_W))
                            .text_color(status_colour(&t, s))
                            .child(s.glyph()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("wsn-{}", s as usize)))
                            .flex_1()
                            .cursor_pointer()
                            .text_color(if kept { t.text } else { t.text2 })
                            .hover(|d| d.text_color(t.text))
                            .child(s.label())
                            .on_click(cx.listener(move |w, _, _, cx| {
                                w.slice.only_status(s);
                                w.reslice();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(size::SMALL))
                            .text_color(t.dim)
                            .child(self.sliced.status_counts[s as usize].to_string()),
                    ),
            );
        }
        div()
            .w(px(RAIL_W))
            .flex_none()
            .h_full()
            .p(px(SPACE_3XL))
            .flex()
            .flex_col()
            .gap(px(SPACE_LG))
            .border_r_1()
            .border_color(t.divider)
            .child(head(
                "Status",
                self.slice.statuses.is_none(),
                "ws-all",
                Rc::new(|w: &mut Work| w.slice.statuses = None),
            ))
            .child(statuses)
            .child(div().h(px(SPACE_MD)))
            .child(head(
                "Project",
                self.slice.projects.is_none(),
                "wp-all",
                Rc::new(|w: &mut Work| w.slice.projects = None),
            ))
            .when(self.board.projects.len() > 6, |d| {
                d.child(self.rail_search.clone())
            })
            .child(
                div()
                    .id("wp-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(projects),
            )
    }

    fn bar(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let shown = self.sliced.rows.len();
        let all = self.board.rows.len();
        let projects = &self.board.projects;
        let loading = projects.iter().any(|p| !p.read && p.error.is_none());
        let loaded = projects
            .iter()
            .filter(|p| p.read && p.error.is_none())
            .count();
        let progress = format!("Reading projects: {loaded} of {} loaded", projects.len());
        div()
            .flex_none()
            .h(px(BAR_H))
            .px(px(SPACE_4XL))
            .flex()
            .items_center()
            .gap(px(SPACE_2XL))
            .border_b_1()
            .border_color(t.divider)
            .child(kit::page_title(&t, "Work"))
            .child(kit::note(
                &t,
                if shown == all {
                    format!("{all} items")
                } else {
                    format!("{shown} of {all}")
                },
                Some(t.text2),
            ))
            .when(loading, |d| {
                d.child(
                    div()
                        .debug_selector({
                            let progress = progress.clone();
                            move || format!("work-loading-{progress}")
                        })
                        .child(kit::note(&t, progress, Some(t.text2))),
                )
            })
            .child(div().flex_1())
            .child(div().w(px(SEARCH_W)).child(self.search.clone()))
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let mut row = div()
            .flex_none()
            .h(px(HEADER_H))
            .px(px(ROW_PAD))
            .flex()
            .items_center()
            .gap(px(GAP))
            .border_b_1()
            .border_color(t.divider)
            .text_size(px(size::SMALL))
            .text_color(t.dim);
        for &c in &self.columns {
            let arrow = match (self.slice.sort == c, self.slice.descending) {
                (true, false) => " ↑",
                (true, true) => " ↓",
                _ => "",
            };
            let cell = div()
                .id(SharedString::from(format!("wh-{}", c.label())))
                .cursor_pointer()
                .whitespace_nowrap()
                .hover(|d| d.text_color(t.text))
                .when(self.slice.sort == c, |d| d.text_color(t.text2))
                .when(right_aligned(c), |d| d.flex().justify_end())
                .child(format!("{}{arrow}", c.label()))
                .on_click(cx.listener(move |w, _, _, cx| w.sort_by(c, cx)));
            row = row.child(match width(c) {
                Some(px_w) => cell.w(px(px_w)).flex_none(),
                None => cell.flex_1().min_w_0(),
            });
        }
        row
    }

    fn row(&mut self, at: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(&i) = self.sliced.rows.get(at) else {
            return div().into_any_element();
        };
        let r = &self.board.rows[i];
        let selected = self.selected_row() == Some(i);
        let debug = format!("work-row-{}", r.id);
        let project = &self.board.projects[r.project];
        let mut row = div()
            .id(("wr", at))
            .debug_selector(move || debug.clone())
            .w_full()
            .h(px(size::ROW + ROW_EXTRA_H))
            .px(px(ROW_PAD))
            .flex()
            .items_center()
            .gap(px(GAP))
            .cursor_pointer()
            .border_l_2()
            .border_color(if selected {
                t.accent
            } else {
                gpui::transparent_black()
            })
            .when(selected, |d| d.bg(t.accent_light()))
            .when(!selected, |d| d.hover(|d| d.bg(alpha(t.text, ROW_HOVER))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                    window.focus(&w.focus, cx);
                    w.select(i, cx);
                    if e.click_count >= 2 {
                        w.card = true;
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                    window.focus(&w.focus, cx);
                    w.select(i, cx);
                    w.host.menu(w.row_menu(i), e.position, cx);
                }),
            );
        let updated = r
            .updated
            .map(|ms| when::age(ms, now_ms()))
            .unwrap_or_default();
        for &c in &self.columns {
            let cell: Div = match c {
                Column::Id => div().text_color(t.text2).child(r.id.clone()),
                Column::Type => div().text_color(t.text2).child(r.kind.clone()),
                Column::Title => div()
                    .text_color(if r.status == Status::Closed {
                        t.text2
                    } else {
                        t.text
                    })
                    .child(r.title.clone()),
                Column::Project => div().text_color(t.text2).child(project.label.clone()),
                Column::Status => div()
                    .flex()
                    .gap(px(SPACE_SM))
                    .child(
                        div()
                            .text_color(status_colour(&t, r.status))
                            .child(r.status.glyph()),
                    )
                    .child(div().text_color(t.text2).child(raw_label(r))),
                Column::Updated => div().text_color(t.text2).child(updated.clone()),
                Column::Size => div().text_color(t.text2).child(size_label(r.size)),
                Column::Kids => {
                    div()
                        .text_color(t.text2)
                        .child(match self.board.links.children[i].len() {
                            0 => String::new(),
                            n => n.to_string(),
                        })
                }
            };
            let cell = cell
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .when(right_aligned(c), |d| d.flex().justify_end());
            row = row.child(match width(c) {
                Some(w) => cell.w(px(w)).flex_none(),
                None => cell.flex_1().min_w_0(),
            });
        }
        row.into_any_element()
    }

    fn table(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let n = self.sliced.rows.len();
        let hidden: usize = STATUSES
            .into_iter()
            .filter(|&s| !self.slice.keeps_status(s))
            .map(|s| self.sliced.status_counts[s as usize])
            .sum();
        let empty: Option<Div> = (n == 0).then(|| {
            let none_read = self.board.projects.iter().all(|p| !p.read);
            let reading = self
                .board
                .projects
                .iter()
                .all(|p| !p.read && p.error.is_none());
            let why = if self.snapshots.is_empty() {
                "syncing…".to_string()
            } else if !self.board.projects.is_empty() && reading {
                format!("Reading {} projects…", self.board.projects.len())
            } else if self.board.projects.is_empty() {
                "No projects are configured on this node: add a `beads` or `github` line to its work.conf.".to_string()
            } else if none_read {
                "No project could be read yet; the footer says why.".to_string()
            } else if hidden > 0 {
                format!("No match here. Matches with other statuses: {hidden}.")
            } else {
                "Nothing in this slice.".to_string()
            };
            div()
                .p(px(SPACE_4XL))
                .flex()
                .items_center()
                .gap(px(GAP))
                .text_color(t.text2)
                .child(why)
                .when(hidden > 0 && !none_read, |d| {
                    d.child(
                        kit::button(&t, "work-every-status", "Show every status", Kind::Plain)
                            .debug_selector(|| "work-every-status".into())
                            .on_click(cx.listener(|w, _, _, cx| {
                                w.slice.statuses = None;
                                w.reslice();
                                cx.notify();
                            })),
                    )
                })
        });
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.header(cx))
            .children(empty)
            .child(
                uniform_list(
                    "work-rows",
                    n,
                    cx.processor(|w, range: std::ops::Range<usize>, _, cx| {
                        range.map(|at| w.row(at, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }

    fn family_links(
        &self,
        t: &Theme,
        label: &str,
        refs: &[String],
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if refs.is_empty() {
            return None;
        }
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_XXS))
            .child(kit::label(t, label));
        for (k, reference) in refs.iter().enumerate() {
            let found = self.board.find(reference).map(|i| &self.board.rows[i]);
            let text = match found {
                Some(r) => format!("{} {} {}", r.status.glyph(), r.id, r.title),
                None => reference.clone(),
            };
            let reference = reference.clone();
            col = col.child(
                div()
                    .id(SharedString::from(format!("fam-{label}-{k}")))
                    .debug_selector({
                        let text = text.clone();
                        let label = label.to_owned();
                        move || format!("work-family-{label}-{text}")
                    })
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(if found.is_some() { t.text } else { t.text2 })
                    .when(found.is_some(), |d| {
                        d.cursor_pointer().hover(|d| d.text_color(t.accent))
                    })
                    .child(text)
                    .on_click(cx.listener(move |w, _, _, cx| w.follow(&reference, cx))),
            );
        }
        Some(col)
    }

    pub(crate) fn card(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        if !self.card {
            return None;
        }
        let i = self.selected_row()?;
        let t = theme(cx).clone();
        let r: &Row = &self.board.rows[i];
        let p = &self.board.projects[r.project];
        let now = now_ms();
        let mut facts = vec![format!("{} {}", r.status.glyph(), raw_label(r))];
        if let Some(pr) = r.priority {
            facts.push(format!("P{pr}"));
        }
        facts.push(r.kind.clone());
        if let Some(c) = r.created {
            facts.push(format!(
                "{}opened {} ago",
                r.author
                    .as_ref()
                    .map(|a| format!("{a} "))
                    .unwrap_or_default(),
                when::age(c, now)
            ));
        }
        if !r.labels.is_empty() {
            facts.push(r.labels.join(" "));
        }
        if !r.assignees.is_empty() {
            facts.push(format!("assigned to {}", r.assignees.join(", ")));
        }
        // The buttons keep their words: past the room they wrap to a line
        // of their own rather than clip.
        let mut head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_XS))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(t.text2)
                    .child(format!("{} · {}", r.id, p.label)),
            );
        if r.url.is_some() {
            head = head.child(
                kit::button(&t, "w-open", "Open on GitHub", Kind::Quiet)
                    .flex_none()
                    .on_click(cx.listener(|w, _, _, cx| w.open_url(cx))),
            );
        }
        if !self.trail.is_empty() {
            head = head.child(
                kit::button(&t, "w-back", "Back", Kind::Quiet)
                    .flex_none()
                    .debug_selector(|| "w-back".into())
                    .tooltip(kit::tip("Alt+Left"))
                    .on_click(cx.listener(|w, _, _, cx| w.back(cx))),
            );
        }
        if p.checkout.is_some() {
            head = head.child(
                kit::button(&t, "w-start", "Start session", Kind::Primary)
                    .flex_none()
                    .debug_selector(|| "work-start".into())
                    .on_click(cx.listener(|w, _, _, cx| w.start_session(cx))),
            );
        }
        head = head
            .child(
                kit::button(&t, "w-send", "Send…", Kind::Quiet)
                    .flex_none()
                    .debug_selector(|| "work-send".into())
                    .tooltip(kit::tip("Alt+S"))
                    .on_click(cx.listener(|w, _, window, cx| w.send(window, cx))),
            )
            .child(
                kit::button(&t, "w-close", "Close", Kind::Quiet)
                    .flex_none()
                    .on_click(cx.listener(|w, _, _, cx| {
                        if let Some(from) = w.beside_from {
                            w.host.close_beside(from, cx);
                        } else {
                            w.card = false;
                            cx.notify();
                        }
                    })),
            );
        let mut pull_facts: Option<Div> = None;
        if let Some(pf) = &r.pull {
            let mut line = div()
                .flex()
                .flex_wrap()
                .gap(px(SPACE_SM))
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
                        .child(format!("· checks {}/{all}", c.passed)),
                );
            }
            pull_facts = Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_XXS))
                    .child(kit::label(&t, "Branch"))
                    .child(line),
            );
        }
        let fam = &r.family;
        let mut family = div().flex().flex_col().gap(px(SPACE_LG));
        let parent: Vec<String> = fam.parent.iter().cloned().collect();
        let none: Vec<String> = vec![];
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
        for (label, refs) in [
            ("Parent", &parent),
            ("Children", &fam.children),
            ("Blocked by", &fam.blockers),
            (
                "Blocking",
                if r.pull.is_some() {
                    &none
                } else {
                    &fam.blocking
                },
            ),
            (pulls_label, pulls),
        ] {
            family = family.children(self.family_links(&t, label, refs, cx));
        }
        let body = prose::link_ids(r.body.trim(), &|id| {
            self.board.find_from(id, r.project).is_some()
        });
        let blocks = markdown::parse(if body.is_empty() {
            "No description."
        } else {
            &body
        });
        let me = cx.entity().downgrade();
        let project = r.project;
        let on_link: markdown::OnLink = Rc::new(move |link: &str, _, cx| {
            let link = link.to_string();
            let _ = me.update(cx, |w, cx| w.follow_link(&link, project, cx));
        });
        let look = markdown::Look {
            t: &t,
            refs: p.tracker == Tracker::Github,
            on_link: Some(on_link),
            select: Some(self.prose.clone()),
        };
        let card = div()
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
            );
        Some(
            select::track(card, &self.prose, cx.entity_id(), |text, cx| {
                select::copy(text, cx)
            })
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(SPACE_4XL))
            .flex()
            .flex_col()
            .gap(px(SPACE_2XL))
            .child(head)
            .child(
                div()
                    .debug_selector({
                        let title = r.title.clone();
                        move || format!("work-card-title-{title}")
                    })
                    .text_size(px(CARD_TITLE_SIZE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(r.title.clone()),
            )
            .child(kit::note(&t, facts.join(" · "), Some(t.text2)))
            .children(pull_facts)
            .child(family)
            .child(kit::label(&t, "Description"))
            .child(markdown::render(&look, "work-body", &blocks)),
        )
    }

    /// Beside the table while a row is selected, or beneath it in a narrow
    /// pane: its family as a graph when it has one, and its card under it.
    fn side(&self, side: Side, cx: &mut Context<Self>) -> Option<Div> {
        let selected = self.selected_row().filter(|_| self.card)?;
        let t = theme(cx).clone();
        let wide = match side {
            Side::Beside(w) => w,
            Side::Beneath => self.pane.get(),
        };
        let pane = div().flex().flex_col().border_color(t.divider);
        let pane = match side {
            Side::Beside(w) => pane.w(px(w)).flex_none().border_l_1(),
            Side::Beneath => pane.flex_1().min_h_0().border_t_1(),
        };
        let graph = crate::store::work::graph::related(&self.board, selected)
            .then(|| self.graph(selected, wide - GRAPH_EDGE_INSET, cx));
        Some(pane.children(graph).children(self.card(cx)))
    }

    /// What was read, what failed and why, a store the node cannot watch,
    /// and when it was last read: failure never reads as no work.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let now = now_ms();
        let mut col = div()
            .flex_none()
            .px(px(SPACE_4XL))
            .py(px(SPACE_SM))
            .flex()
            .flex_col()
            .gap(px(SPACE_XXS))
            .border_t_1()
            .border_color(t.divider)
            .bg(t.surface)
            .text_size(px(FOOTER_TEXT_SIZE));
        if self.snapshots.is_empty() {
            return col.child(kit::note(&t, "syncing…", Some(t.text2)));
        }
        let read: Vec<&str> = self
            .board
            .projects
            .iter()
            .filter(|p| p.read && p.error.is_none())
            .map(|p| p.label.as_str())
            .collect();
        // Beads stores follow their own changes; GitHub is read on asking.
        let github = self
            .board
            .projects
            .iter()
            .filter(|p| p.tracker == Tracker::Github)
            .filter_map(|p| p.synced)
            .max();
        let mut summary = format!(
            "{} of {} projects read",
            read.len(),
            self.board.projects.len()
        );
        if let Some(at) = github {
            summary.push_str(&format!(" · GitHub read {} ago", when::age(at, now)));
        }
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_MD))
                .child(div().text_color(t.text2).child(summary))
                .child(if self.refreshing.is_empty() {
                    kit::button(&t, "w-refresh", "Refresh", Kind::Quiet)
                        .text_size(px(FOOTER_TEXT_SIZE))
                        .tooltip(kit::tip("Read GitHub and the Beads stores again now"))
                        .on_click(cx.listener(|w, _, _, cx| w.refresh(cx)))
                        .into_any_element()
                } else {
                    div()
                        .debug_selector(|| "w-refreshing".into())
                        .text_color(t.accent)
                        .child("Reading every project again…")
                        .into_any_element()
                }),
        );
        for p in &self.board.projects {
            if let Some(e) = &p.error {
                let old = if p.read {
                    format!(
                        " (showing the read from {} ago)",
                        p.synced.map(|s| when::age(s, now)).unwrap_or_default()
                    )
                } else {
                    String::new()
                };
                let fix = match p.tracker {
                    Tracker::Github => " · check with: gh auth status",
                    Tracker::Beads => " · check with: bd list (in the folder)",
                };
                col = col.child(
                    div()
                        .text_color(t.error)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(format!("{} not read: {}{old}{fix}", p.label, e.trim())),
                );
            }
            if let Some(why) = &p.unwatched {
                col = col.child(
                    div()
                        .text_color(t.error)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(format!(
                            "{} is not watched: {} · its Beads are as read when watching began",
                            p.label,
                            why.trim()
                        )),
                );
            }
        }
        col
    }
}

impl Work {
    /// Keeps the pane's width as drawn, the bar's being the pane's, and
    /// draws again when it changed: a window resized, the column docked.
    fn measure(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(Vec<gpui::Bounds<gpui::Pixels>>, &mut Window, &mut gpui::App) + 'static {
        let (pane, me) = (self.pane.clone(), cx.entity().downgrade());
        move |bounds, window, cx| {
            let Some(bar) = bounds.first() else {
                return;
            };
            let w = bar.size.width / px(1.);
            if (w - pane.get()).abs() < RESIZE_THRESHOLD {
                return;
            }
            pane.set(w);
            let me = me.clone();
            window.defer(cx, move |_, cx| {
                let _ = me.update(cx, |_, cx| cx.notify());
            });
        }
    }
}

/// The tracker's own status word, as the operator knows it.
fn raw_label(r: &Row) -> String {
    match (r.status, r.raw_status.as_str()) {
        (Status::Blocked, "open") => "blocked".into(),
        (_, raw) => raw.replace('_', " "),
    }
}

impl Render for Work {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        if self.pane.get() == 0. {
            // Until the first frame measures it: the window less the rail.
            self.pane
                .set(window.viewport_size().width / px(1.) - RAIL_W);
        }
        let side = fit::side(self.pane.get());
        let table = match side {
            Side::Beside(w) if self.card && self.selected_row().is_some() => self.pane.get() - w,
            _ => self.pane.get(),
        };
        self.columns = fit::columns(table);
        div()
            .id("work")
            .key_context("Work")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .font_family(FAMILY)
            .on_key_down(cx.listener(|w, e: &gpui::KeyDownEvent, window, cx| {
                if w.focus.is_focused(window) {
                    w.key(e, window, cx);
                } else {
                    w.alt_key(e, window, cx);
                }
            }))
            .child(
                div().flex_1().min_h_0().flex().child(self.rail(cx)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(self.bar(cx))
                        .child(self.strip(cx))
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .when(side == Side::Beneath, |d| d.flex_col())
                                .child(self.table(cx))
                                .children(self.side(side, cx)),
                        )
                        .on_children_prepainted(self.measure(cx)),
                ),
            )
            .child(self.footer(cx))
    }
}
