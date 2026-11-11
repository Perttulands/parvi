//! Channel state and the selected mission, independent of the page's views.
pub mod attempts;
pub mod canvas;
pub mod flow;
pub mod gate;
pub mod placement;
pub mod run_state;
pub mod staffing;
pub mod undo;
pub mod words;

use parvi_missions_contract::{
    Layout, Mission, MissionListing, MissionSnapshot, RUNS_SNAPSHOT, RunProjection, SNAPSHOT,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Feed {
    pub missions: Option<Vec<MissionListing>>,
    pub runs: BTreeMap<String, RunProjection>,
}
#[derive(Default)]
pub struct Store {
    pub nodes: BTreeMap<String, Feed>,
    pub selected: Option<(String, String)>,
    pub mission: Option<Mission>,
    pub mission_etag: Option<String>,
    pub layout_etag: Option<String>,
    pub layout: Option<Layout>,
    pub error: Option<String>,
}
impl Store {
    pub fn frame(&mut self, node: &str, kind: u8, body: &[u8]) {
        let feed = self.nodes.entry(node.to_owned()).or_default();
        match kind {
            SNAPSHOT => {
                if let Ok(snapshot) = serde_json::from_slice::<MissionSnapshot>(body) {
                    feed.missions = Some(snapshot.missions);
                }
            }
            RUNS_SNAPSHOT => {
                if let Ok(runs) = serde_json::from_slice::<Vec<RunProjection>>(body) {
                    for run in runs {
                        if feed
                            .runs
                            .get(&run.run_id)
                            .is_none_or(|old| run.event_count >= old.event_count)
                        {
                            feed.runs.insert(run.run_id.clone(), run);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_older_projection_never_replaces_a_newer_one() {
        let mut store = Store::default();
        let mut run = json!({"runId":"r", "status":"running", "final":false, "resumeAllowed":false,
          "epoch":1, "missionSlug":"hello", "missionId":"m", "missionRev":1,
          "inputCardId":"i", "work":"", "cwd":"", "repository":"", "base":"",
          "requestId":"", "contextPaths":[], "inputs":[], "outputs":[], "eventCount":9,
          "startedBy":"", "startedAt":"", "updatedAt":"", "endedBy":"", "waitingGates":[], "events":[]});
        store.frame(
            "node",
            RUNS_SNAPSHOT,
            &serde_json::to_vec(&vec![&run]).unwrap(),
        );
        run["eventCount"] = json!(3);
        run["status"] = json!("blocked");
        store.frame(
            "node",
            RUNS_SNAPSHOT,
            &serde_json::to_vec(&vec![&run]).unwrap(),
        );
        assert_eq!(store.nodes["node"].runs["r"].status, "running");
    }
}
