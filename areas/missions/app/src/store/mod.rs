//! Channel state and the selected mission, independent of the page's views.
pub mod attempts;
pub mod canvas;
pub mod evidence;
pub mod flow;
pub mod gate;
pub mod navigator;
pub mod placement;
pub mod run_state;
pub mod seats;
pub mod staffing;
pub mod undo;
pub mod validation;
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
    /// Resolves a reveal on a named node, or across nodes in live-node order.
    pub fn find_reveal(
        &self,
        what: &str,
        id: &str,
        node: Option<&str>,
        live: &[String],
    ) -> Option<(String, String)> {
        let mut nodes = if let Some(node) = node {
            vec![node.to_owned()]
        } else {
            live.to_vec()
        };
        if node.is_none() {
            for n in self.nodes.keys() {
                if !nodes.contains(n) {
                    nodes.push(n.clone());
                }
            }
        }
        nodes.into_iter().find_map(|n| {
            let feed = self.nodes.get(&n)?;
            match what {
                "mission" => feed
                    .missions
                    .as_ref()?
                    .iter()
                    .any(|m| m.slug == id)
                    .then(|| (n, id.to_owned())),
                "run" => feed.runs.get(id).map(|r| (n, r.mission_slug.clone())),
                _ => None,
            }
        })
    }
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
    fn reveals_use_named_node_or_serving_node_first() {
        let mut s = Store::default();
        let listed = |title: &str| {
            vec![MissionListing {
                slug: "same".into(),
                id: title.into(),
                title: title.into(),
                rev: Some(1),
                steps: 0,
                broken: None,
            }]
        };
        s.nodes.insert(
            "remote".into(),
            Feed {
                missions: Some(listed("Remote")),
                ..Default::default()
            },
        );
        s.nodes.insert(
            "local".into(),
            Feed {
                missions: Some(listed("Local")),
                ..Default::default()
            },
        );
        let run: RunProjection = serde_json::from_value(json!({"runId":"r", "status":"running", "final":false, "resumeAllowed":false,"epoch":1,"missionSlug":"same","missionId":"m","missionRev":1,"inputCardId":"i","work":"","cwd":"","repository":"","base":"","contextPaths":[],"inputs":[],"outputs":[],"eventCount":1,"startedBy":"","startedAt":"","updatedAt":"","waitingGates":[],"events":[]})).unwrap();
        s.nodes
            .get_mut("remote")
            .unwrap()
            .runs
            .insert("r".into(), run);
        assert_eq!(
            s.find_reveal("mission", "same", None, &["local".into(), "remote".into()]),
            Some(("local".into(), "same".into()))
        );
        assert_eq!(
            s.find_reveal("mission", "same", Some("remote"), &["local".into()]),
            Some(("remote".into(), "same".into()))
        );
        assert_eq!(
            s.find_reveal("run", "r", None, &["local".into()]),
            Some(("remote".into(), "same".into()))
        );
        assert_eq!(
            s.find_reveal("run", "r", Some("local"), &["remote".into()]),
            None
        );
    }
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
