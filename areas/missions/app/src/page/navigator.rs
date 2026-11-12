//! The mission navigator: needs-you, searchable mission groups and run rows.
use super::*;
use crate::store::{navigator as model, words};
use gpui::{FontWeight, SharedString, Stateful};
use parvi_app_kit::theme::Theme;
use std::collections::HashSet;

/// Runs shown in each mission before the operator expands its history.
const LATEST: usize = 5;

pub(super) struct Navigator {
    filter: Entity<TextField>,
    folded: HashSet<String>,
    expanded: HashSet<String>,
}
impl Navigator {
    pub(super) fn new(cx: &mut Context<MissionsPage>) -> Self {
        let filter = cx.new(|cx| TextField::new(cx, "Filter missions and runs"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        Self {
            filter,
            folded: HashSet::new(),
            expanded: HashSet::new(),
        }
    }
}
fn heading(t: &Theme, id: String, open: bool, title: String, count: usize) -> Stateful<Div> {
    let selector = id.clone();
    let hover = t.accent_soft();
    div()
        .id(SharedString::from(id))
        .debug_selector(move || selector.clone())
        .flex()
        .items_center()
        .gap(px(TAB_GAP))
        .h(px(size::ROW))
        .px(px(PAD))
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .child(
            div()
                .flex_none()
                .text_size(px(size::SMALL))
                .text_color(t.text2)
                .child(if open { "▼" } else { "▶" }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(size::TEXT))
                .font_weight(FontWeight::BOLD)
                .child(title),
        )
        .when(count > 0, |d| {
            d.child(
                div()
                    .flex_none()
                    .px(px(TAB_GAP))
                    .rounded_full()
                    .bg(t.raised)
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .child(count.to_string()),
            )
        })
}
impl MissionsPage {
    fn navigator_row(
        &self,
        t: &Theme,
        row: &model::Row,
        under: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selected = self
            .state
            .selected
            .as_ref()
            .is_some_and(|(node, slug)| node == &row.node && slug == &row.run.mission_slug)
            && self
                .selected_run()
                .is_some_and(|run| run.run_id == row.run.run_id);
        let (node, slug, id) = (
            row.node.clone(),
            row.run.mission_slug.clone(),
            row.run.run_id.clone(),
        );
        let duplicate = under == "group" && words::needs_you(&row.run);
        let selector = if duplicate {
            format!("run-group-row-{id}")
        } else {
            format!("run-row-{id}")
        };
        let element_id = format!("nav-{under}-{node}-{id}");
        let now = time::OffsetDateTime::now_utc();
        let colour = if words::needs_you(&row.run) {
            t.error
        } else {
            t.text2
        };
        let hover = t.accent_soft();
        let subtitle = if under == "needs" {
            if row.about.is_empty() {
                title.to_owned()
            } else {
                format!("{title} · {}", row.about)
            }
        } else {
            row.about.clone()
        };
        div()
            .id(SharedString::from(element_id))
            .debug_selector(move || selector.clone())
            .flex()
            .flex_col()
            .px(px(PAD))
            .py(px(TAB_GAP))
            .border_l_2()
            .border_color(if selected {
                t.accent
            } else {
                gpui::transparent_black()
            })
            .when(selected, |d| d.bg(t.accent_light()))
            .hover(move |d| d.bg(hover))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(TAB_GAP))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Run {}", row.number)),
                    )
                    .child(
                        div()
                            .text_size(px(size::SMALL))
                            .text_color(t.text2)
                            .child(words::short_time(&row.run.started_at, now)),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(size::SMALL))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colour)
                            .child(words::state_word(&row.run.status).to_uppercase()),
                    ),
            )
            .when(!subtitle.is_empty(), |d| {
                d.child(
                    div()
                        .truncate()
                        .text_size(px(size::SMALL))
                        .text_color(t.text2)
                        .child(subtitle),
                )
            })
            .on_click(cx.listener(move |page, _, _, cx| {
                if page.state.selected.as_ref() != Some(&(node.clone(), slug.clone())) {
                    page.open(node.clone(), slug.clone(), cx);
                }
                if page.state.selected.as_ref() == Some(&(node.clone(), slug.clone())) {
                    page.show_run(id.clone(), cx);
                }
            }))
    }
    pub(super) fn render_navigator(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let q = self.navigator.filter.read(cx).text.trim().to_lowercase();
        let groups = model::groups(&self.state);
        let mut list = div().flex().flex_col().gap(px(TAB_GAP)).child(
            div()
                .debug_selector(|| "mission-filter".into())
                .child(self.navigator.filter.clone()),
        );
        if self
            .state
            .nodes
            .values()
            .all(|feed| feed.missions.is_none())
        {
            list = list.child(
                div()
                    .p(px(PAD))
                    .text_color(t.dim)
                    .child("Reading missions…"),
            );
        }
        let mut needing: Vec<_> = groups
            .iter()
            .flat_map(|g| {
                g.runs
                    .iter()
                    .filter(|r| words::needs_you(&r.run) && model::matches(&q, r))
                    .map(move |r| (g, r))
            })
            .collect();
        needing.sort_by(|(_, a), (_, b)| {
            let rank = |status: &str| if status == "waiting_human" { 0 } else { 1 };
            rank(&a.run.status)
                .cmp(&rank(&b.run.status))
                .then_with(|| b.run.started_at.cmp(&a.run.started_at))
        });
        if !needing.is_empty() {
            let open = !self.navigator.folded.contains("needs");
            list = list.child(
                heading(
                    t,
                    "needs-you-group".into(),
                    open,
                    "Needs you".into(),
                    needing.len(),
                )
                .on_click(cx.listener(|page, _, _, cx| {
                    fold(&mut page.navigator.folded, "needs");
                    cx.notify();
                })),
            );
            if open {
                for (group, row) in needing {
                    list = list.child(self.navigator_row(t, row, "needs", &group.title, cx));
                }
            }
        }
        for group in &groups {
            let key = format!("{}/{}", group.node, group.slug);
            let title_hit = q.is_empty()
                || group.title.to_lowercase().contains(&q)
                || group.slug.to_lowercase().contains(&q);
            let runs: Vec<_> = group
                .runs
                .iter()
                .filter(|r| title_hit || model::matches(&q, r))
                .collect();
            if !title_hit && runs.is_empty() {
                continue;
            }
            let open = !q.is_empty() || !self.navigator.folded.contains(&key);
            let fold_key = key.clone();
            let node = group.node.clone();
            let slug = group.slug.clone();
            let selector = format!("mission-{node}-{slug}");
            let fold_selector = format!("mission-fold-{key}");
            let group_selector = format!("mission-group-{key}");
            let hover = t.accent_soft();
            list = list.child(
                div()
                    .id(SharedString::from(group_selector.clone()))
                    .debug_selector(move || group_selector.clone())
                    .flex()
                    .items_center()
                    .gap(px(TAB_GAP))
                    .h(px(size::ROW))
                    .px(px(PAD))
                    .hover(move |d| d.bg(hover))
                    .child(
                        div()
                            .id(SharedString::from(fold_selector.clone()))
                            .debug_selector(move || fold_selector.clone())
                            .flex_none()
                            .w(px(PAD * 2.))
                            .h(px(size::ROW))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(size::SMALL))
                            .text_color(t.text2)
                            .child(if open { "▼" } else { "▶" })
                            .on_click(cx.listener(move |page, _, _, cx| {
                                fold(&mut page.navigator.folded, &fold_key);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .text_size(px(size::TEXT))
                            .font_weight(FontWeight::BOLD)
                            .child(group.title.clone())
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.open(node.clone(), slug.clone(), cx)
                            })),
                    )
                    .when(!group.runs.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px(px(TAB_GAP))
                                .rounded_full()
                                .bg(t.raised)
                                .text_size(px(size::SMALL))
                                .text_color(t.text2)
                                .child(group.runs.len().to_string()),
                        )
                    }),
            );
            if !open {
                continue;
            }
            if self.state.selected.as_ref() == Some(&(group.node.clone(), group.slug.clone()))
                && self.state.mission.is_some()
                && self.state.mission_etag.is_some()
            {
                let rename = format!("mission-rename-{}-{}", group.node, group.slug);
                let delete = format!("mission-delete-{}-{}", group.node, group.slug);
                list = list.child(
                    div()
                        .flex()
                        .px(px(PAD))
                        .gap(px(TAB_GAP))
                        .child(
                            kit::button(
                                t,
                                SharedString::from(rename.clone()),
                                "Rename",
                                Kind::Quiet,
                            )
                            .debug_selector(move || rename.clone())
                            .on_click(cx.listener(
                                |page, _, window, cx| page.open_mission_rename(window, cx),
                            )),
                        )
                        // Delete keeps its distance from Rename.
                        .child(div().flex_1())
                        .child(
                            kit::button(
                                t,
                                SharedString::from(delete.clone()),
                                "Delete",
                                Kind::Quiet,
                            )
                            .text_color(t.error)
                            .debug_selector(move || delete.clone())
                            .on_click(cx.listener(
                                |page, _, window, cx| page.open_mission_archive(window, cx),
                            )),
                        ),
                );
            }
            if runs.is_empty() {
                list = list.child(div().px(px(PAD)).text_color(t.dim).child("No runs yet."));
                continue;
            }
            list = list.child(div().px(px(PAD)).child(kit::label(t, "Runs")));
            let n = if self.navigator.expanded.contains(&key) || !q.is_empty() {
                runs.len()
            } else {
                LATEST.min(runs.len())
            };
            for row in &runs[..n] {
                list = list.child(self.navigator_row(t, row, "group", &group.title, cx));
            }
            if runs.len() > n {
                let selector = format!("mission-more-{key}");
                let more = runs.len() - n;
                list = list.child(
                    kit::button(
                        t,
                        SharedString::from(selector.clone()),
                        format!("{more} earlier runs"),
                        Kind::Quiet,
                    )
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.navigator.expanded.insert(key.clone());
                        cx.notify();
                    })),
                );
            }
        }
        if !self.state.nodes.is_empty()
            && self
                .state
                .nodes
                .values()
                .any(|feed| feed.missions.is_some())
            && groups.is_empty()
        {
            list = list.child(
                div()
                    .p(px(PAD))
                    .text_color(t.dim)
                    .child("No missions on this node yet."),
            );
        } else if !q.is_empty()
            && !groups.iter().any(|g| {
                g.title.to_lowercase().contains(&q)
                    || g.slug.to_lowercase().contains(&q)
                    || g.runs.iter().any(|r| model::matches(&q, r))
            })
        {
            list = list.child(
                div()
                    .p(px(PAD))
                    .text_color(t.dim)
                    .child("No matching missions or runs."),
            );
        }
        list
    }
}
fn fold(set: &mut HashSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.into());
    }
}
