//! Keep Run's emphasis tied to a current node-side validation result.
use super::*;
use crate::store::validation::Key;
use parvi_missions_contract::{Mission, ValidationResponse};

impl MissionsPage {
    fn validation_key(&self) -> Option<Key> {
        let (node, slug) = self.state.selected.as_ref()?;
        let mission = self.state.mission.as_ref()?;
        Some(Key {
            node: node.clone(),
            slug: slug.clone(),
            rev: mission.rev,
            etag: self.state.mission_etag.clone()?,
        })
    }
    /// Errors and warnings for the saved, current mission. Never paint an old
    /// report on a changed definition or on a run's frozen canvas.
    pub(super) fn canvas_findings(&self) -> Vec<(parvi_missions_contract::Finding, bool)> {
        if self.writing || self.is_showing_run() {
            return Vec::new();
        }
        let Some(key) = self.validation_key() else {
            return Vec::new();
        };
        if self
            .state
            .nodes
            .get(&key.node)
            .and_then(|feed| feed.missions.as_ref())
            .and_then(|list| list.iter().find(|item| item.slug == key.slug))
            .and_then(|item| item.rev)
            .is_some_and(|rev| rev != key.rev)
        {
            return Vec::new();
        }
        let Some(report) = self.validation.report(&key) else {
            return Vec::new();
        };
        report
            .errors
            .iter()
            .cloned()
            .map(|finding| (finding, true))
            .chain(
                report
                    .warnings
                    .iter()
                    .cloned()
                    .map(|finding| (finding, false)),
            )
            .collect()
    }
    /// What stands between the mission and a run: its errors, and the
    /// warning that no Input leads anywhere.
    pub(super) fn todo(
        &self,
        mission: &Mission,
    ) -> Vec<(String, Vec<parvi_missions_contract::Finding>)> {
        crate::store::validation::todo_lines(
            mission,
            self.canvas_findings()
                .into_iter()
                .filter(|(finding, error)| {
                    *error || finding.code == parvi_missions_contract::Code::MissionNotRunnable
                })
                .map(|(finding, _)| finding),
        )
    }
    /// What the mission still lacks, below the canvas so it never covers a
    /// card: one plain line per thing to do, gone once it is done. In red
    /// once a run was asked for and could not start.
    pub(super) fn todo_band(
        &self,
        t: &parvi_app_kit::theme::Theme,
        mission: &Mission,
    ) -> Option<Div> {
        let lines = self.todo(mission);
        if lines.is_empty() {
            return None;
        }
        let color = if self.run_tried { t.error } else { t.text };
        let heading = format!(
            "{} {} before it can run",
            lines.len(),
            if self.run_tried { "to fix" } else { "to do" }
        );
        let mut list = div()
            .debug_selector(|| "canvas-findings-list".into())
            .flex()
            .flex_col();
        for (line, found) in lines {
            let shown = format!("validation-display-{line}");
            let mut row = div()
                .debug_selector(move || shown.clone())
                .text_color(color)
                .child(line);
            for finding in found {
                let selector =
                    format!("validation-message-{}-{}", finding.node_id, finding.message);
                row = row.child(div().debug_selector(move || selector.clone()));
            }
            list = list.child(row);
        }
        Some(
            div()
                .debug_selector(|| "canvas-findings".into())
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(size::RADIUS))
                .px(px(PAD))
                .py(px(TAB_GAP))
                .border_t_1()
                .border_color(t.divider)
                .bg(t.surface)
                .child(
                    div()
                        .text_color(if self.run_tried { t.error } else { t.text2 })
                        .child(heading),
                )
                .child(list),
        )
    }
    pub(super) fn mission_input_runnable(&self, input: &str) -> bool {
        if self.writing || self.is_showing_run() {
            return false;
        }
        let Some(key) = self.validation_key() else {
            return false;
        };
        if !self
            .state
            .mission
            .as_ref()
            .is_some_and(|m| m.input_cards.iter().any(|card| card.id == input))
        {
            return false;
        }
        if self
            .state
            .nodes
            .get(&key.node)
            .and_then(|feed| feed.missions.as_ref())
            .and_then(|list| list.iter().find(|item| item.slug == key.slug))
            .and_then(|item| item.rev)
            .is_some_and(|rev| rev != key.rev)
        {
            return false;
        }
        self.validation.runnable(&key, input)
    }
    pub(super) fn mission_run_ready(&self) -> bool {
        self.state
            .mission
            .as_ref()
            .and_then(|m| m.input_cards.first())
            .is_some_and(|input| self.mission_input_runnable(&input.id))
    }
    pub(super) fn validation_complete(&self) -> bool {
        self.validation_key()
            .is_some_and(|key| self.validation.complete(&key))
    }
    pub(super) fn validation_failed(&self) -> bool {
        self.validation_key()
            .is_some_and(|key| self.validation.failed(&key))
    }
    pub(super) fn sync_validation(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.validation_key() else {
            self.validation.clear();
            return;
        };
        if !self
            .host
            .nodes(cx)
            .iter()
            .any(|node| node.id.as_str() == key.node)
        {
            self.validation.clear();
            return;
        }
        let Some(request_generation) = self.validation.begin(key.clone()) else {
            return;
        };
        let path = format!("{}/validation", mission_path(&key.slug));
        let asked = self
            .host
            .request(Request::get(NodeId(key.node.clone()), path), cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation || page.validation_key().as_ref() != Some(&key) {
                    return;
                }
                let report = match answer {
                    Ok(answer) if answer.ok() => {
                        serde_json::from_slice::<ValidationResponse>(&answer.body)
                            .map_err(|e| e.to_string())
                    }
                    Ok(answer) => Err(answer.error_text()),
                    Err(error) => Err(error.message),
                };
                page.validation.finish(&key, request_generation, report);
                cx.notify();
            });
        })
        .detach();
    }
}
