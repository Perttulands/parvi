//! The list: runs that need the operator, what is running, what finished
//! lately, then the missions to set off.
use super::*;
use crate::store::{navigator as model, words};
use gpui::{SharedString, rgb};
use parvi_app_kit::theme::Theme;
use parvi_app_kit::widgets::pane;

/// Finished runs the list keeps; older ones stay under their mission.
const RECENT: usize = 8;

pub(super) struct Navigator {
    filter: Entity<TextField>,
}
impl Navigator {
    pub(super) fn new(cx: &mut Context<MissionsPage>) -> Self {
        let filter = cx.new(|cx| TextField::new(cx, "Filter"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        Self { filter }
    }
}

/// What the list shows, in its order: each run under the group it stands
/// in, then the missions.
pub(super) struct Listing {
    pub needs: Vec<(model::Row, String)>,
    pub running: Vec<(model::Row, String)>,
    pub finished: Vec<(model::Row, String)>,
    pub missions: Vec<model::Group>,
}

impl Listing {
    pub(super) fn new(store: &crate::store::Store, query: &str) -> Self {
        let groups = model::groups(store);
        let mut needs = Vec::new();
        let mut running = Vec::new();
        let mut finished = Vec::new();
        for group in &groups {
            let title_hit = query.is_empty()
                || group.title.to_lowercase().contains(query)
                || group.slug.to_lowercase().contains(query);
            for row in &group.runs {
                if !(title_hit || model::matches(query, row)) {
                    continue;
                }
                let entry = (row.clone(), group.title.clone());
                if words::needs_you(&row.run) {
                    needs.push(entry);
                } else if !row.run.r#final {
                    running.push(entry);
                } else {
                    finished.push(entry);
                }
            }
        }
        let newest = |a: &(model::Row, String), b: &(model::Row, String)| {
            words::parse_time(&b.0.run.updated_at).cmp(&words::parse_time(&a.0.run.updated_at))
        };
        needs.sort_by(newest);
        running.sort_by(newest);
        finished.sort_by(newest);
        if query.is_empty() {
            finished.truncate(RECENT);
        }
        let missions = groups
            .into_iter()
            .filter(|g| {
                query.is_empty()
                    || g.title.to_lowercase().contains(query)
                    || g.slug.to_lowercase().contains(query)
            })
            .collect();
        Self {
            needs,
            running,
            finished,
            missions,
        }
    }

    /// The first thing in the list, which the page shows when nothing is
    /// chosen: `(node, slug, run)`.
    pub(super) fn first(&self) -> Option<(String, String, Option<String>)> {
        self.needs
            .iter()
            .chain(&self.running)
            .chain(&self.finished)
            .map(|(row, _)| {
                (
                    row.node.clone(),
                    row.run.mission_slug.clone(),
                    Some(row.run.run_id.clone()),
                )
            })
            .next()
            .or_else(|| {
                self.missions
                    .first()
                    .map(|g| (g.node.clone(), g.slug.clone(), None))
            })
    }
}

impl MissionsPage {
    fn run_row(
        &self,
        t: &Theme,
        row: &model::Row,
        title: &str,
        now: time::OffsetDateTime,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let chosen = self
            .selected_run()
            .is_some_and(|run| run.run_id == row.run.run_id)
            || self.run.selected.as_deref() == Some(row.run.run_id.as_str());
        let (node, slug, id) = (
            row.node.clone(),
            row.run.mission_slug.clone(),
            row.run.run_id.clone(),
        );
        let selector = format!("run-row-{id}");
        let (meta, colour) = match row.run.status.as_str() {
            "waiting_human" => ("your answer".to_owned(), Some(rgb(t.ansi[3]).into())),
            "blocked" => ("blocked".to_owned(), Some(rgb(t.ansi[3]).into())),
            "failed" => ("failed".to_owned(), Some(t.error)),
            "canceled" => ("stopped".to_owned(), None),
            _ if !row.run.r#final => (words::run_span(&row.run, now), Some(t.accent)),
            _ => (words::short_time(&row.run.updated_at, now), None),
        };
        pane::row(t, SharedString::from(format!("nav-{node}-{id}")), chosen)
            .debug_selector(move || selector.clone())
            .tooltip(kit::tip(format!("Run {} · {}", row.number, row.about)))
            .child(pane::cell(title.to_owned()))
            .child(pane::meta(t, meta, colour))
            .on_click(cx.listener(move |page, _, _, cx| {
                page.choose_run(node.clone(), slug.clone(), id.clone(), cx)
            }))
    }

    /// Show a run: open its mission, then the run in it.
    pub(super) fn choose_run(
        &mut self,
        node: String,
        slug: String,
        id: String,
        cx: &mut Context<Self>,
    ) {
        if self.state.selected.as_ref() != Some(&(node.clone(), slug.clone())) {
            self.open(node.clone(), slug.clone(), cx);
        }
        if self.state.selected.as_ref() == Some(&(node, slug)) {
            self.mission_canvas_mode = false;
            self.show_run(id, cx);
        }
    }

    /// With nothing chosen, show the first thing in the list, so the page
    /// answers what is going on without a click.
    pub(super) fn choose_first(&mut self, cx: &mut Context<Self>) {
        if self.state.selected.is_some()
            || self.run.selected.is_some()
            || self.wanted.is_some()
            || self.manage_is_open()
            || self.state.error.is_some()
        {
            return;
        }
        let query = self.navigator.filter.read(cx).text.trim().to_lowercase();
        match Listing::new(&self.state, &query).first() {
            Some((node, slug, Some(run))) => self.choose_run(node, slug, run, cx),
            Some((node, slug, None)) => self.choose_mission(node, slug, cx),
            None => {}
        }
    }

    /// Show a mission as it is defined.
    pub(super) fn choose_mission(&mut self, node: String, slug: String, cx: &mut Context<Self>) {
        let target = Some((node.clone(), slug.clone()));
        if self.state.selected != target || self.run.selected.is_some() || self.run.starting {
            if self.state.selected == target && self.run_unsaved(cx).is_none() {
                self.run = run::RunUi::new(cx);
                cx.notify();
            } else {
                self.open(node, slug, cx);
            }
        }
    }

    pub(super) fn render_navigator(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let query = self.navigator.filter.read(cx).text.trim().to_lowercase();
        let listing = Listing::new(&self.state, &query);
        let now = time::OffsetDateTime::now_utc();
        let head = pane::list_head(
            t,
            Some(
                div()
                    .debug_selector(|| "mission-filter".into())
                    .child(self.navigator.filter.clone())
                    .into_any_element(),
            ),
        )
        .child(
            pane::act(t, "new-mission", "New")
                .debug_selector(|| "new-mission".into())
                .tooltip(kit::tip("Define a new mission"))
                .on_click(cx.listener(|p, _, window, cx| p.open_mission_create(window, cx))),
        );
        let mut rows = div().flex().flex_col();
        let reading = self
            .state
            .nodes
            .values()
            .all(|feed| feed.missions.is_none());
        if reading {
            rows = rows.child(pane::label(t, "Reading missions…"));
            return pane::list(t, "missions-list", head, rows)
                .debug_selector(|| "missions-rail".into());
        }
        if !listing.needs.is_empty() {
            rows =
                rows.child(pane::label(t, "Needs you").debug_selector(|| "needs-you-group".into()));
            for (row, title) in &listing.needs {
                rows = rows.child(self.run_row(t, row, title, now, cx));
            }
        }
        rows = rows.child(pane::label(t, "Running"));
        if listing.running.is_empty() {
            rows = rows.child(
                div()
                    .debug_selector(|| "nothing-running".into())
                    .px(px(size::ROW_PAD))
                    .min_h(px(size::ROW))
                    .flex()
                    .items_center()
                    .text_color(t.dim)
                    .child("Nothing running"),
            );
        }
        for (row, title) in &listing.running {
            rows = rows.child(self.run_row(t, row, title, now, cx));
        }
        if !listing.finished.is_empty() {
            rows = rows.child(pane::label(t, "Finished"));
            for (row, title) in &listing.finished {
                rows = rows.child(self.run_row(t, row, title, now, cx));
            }
        }
        rows = rows.child(pane::label(t, "Missions"));
        if listing.missions.is_empty() {
            rows = rows.child(div().px(px(size::ROW_PAD)).text_color(t.dim).child(
                if query.is_empty() {
                    "None yet"
                } else {
                    "None match"
                },
            ));
        }
        for group in &listing.missions {
            let (node, slug) = (group.node.clone(), group.slug.clone());
            let chosen = self.state.selected.as_ref() == Some(&(node.clone(), slug.clone()))
                && self.run.selected.is_none()
                && !self.run.starting;
            let selector = format!("mission-{node}-{slug}");
            let (meta, colour) = if group.broken.is_some() {
                ("unreadable", Some(t.error))
            } else if group.steps == 0 {
                ("empty", None)
            } else {
                ("", None)
            };
            rows = rows.child(
                pane::row(
                    t,
                    SharedString::from(format!("nav-mission-{node}-{slug}")),
                    chosen,
                )
                .debug_selector(move || selector.clone())
                .when_some(group.broken.clone(), |d, why| d.tooltip(kit::tip(why)))
                .child(pane::cell(group.title.clone()))
                .child(pane::meta(t, meta, colour))
                .on_click(cx.listener(move |page, _, _, cx| {
                    page.choose_mission(node.clone(), slug.clone(), cx)
                })),
            );
        }
        pane::list(t, "missions-list", head, rows).debug_selector(|| "missions-rail".into())
    }
}
