//! Mission navigator rows: groups, stable run numbering and attention order.
use super::{Store, words};
use parvi_missions_contract::RunProjection;

#[derive(Clone)]
pub struct Row {
    pub node: String,
    pub run: RunProjection,
    pub number: usize,
    pub about: String,
}
#[derive(Clone)]
pub struct Group {
    pub node: String,
    pub slug: String,
    pub title: String,
    pub runs: Vec<Row>,
}
fn attention(status: &str) -> u8 {
    match status {
        "waiting_human" => 0,
        "running" => 1,
        "blocked" => 2,
        _ => 3,
    }
}
pub fn groups(store: &Store) -> Vec<Group> {
    let mut groups = Vec::new();
    for (node, feed) in &store.nodes {
        for mission in feed.missions.iter().flatten() {
            let mut runs: Vec<_> = feed
                .runs
                .values()
                .filter(|r| r.mission_slug == mission.slug)
                .cloned()
                .collect();
            runs.sort_by(|a, b| {
                let key = |run: &RunProjection| {
                    words::parse_time(&run.started_at).map(|time| time.unix_timestamp_nanos())
                };
                key(a).cmp(&key(b)).then_with(|| a.run_id.cmp(&b.run_id))
            });
            let mut runs: Vec<_> = runs
                .into_iter()
                .enumerate()
                .map(|(index, run)| {
                    let about = if run.work.is_empty() {
                        words::input_excerpt(&run)
                    } else {
                        words::work_label(&run.work)
                    };
                    Row {
                        node: node.clone(),
                        run,
                        number: index + 1,
                        about,
                    }
                })
                .collect();
            runs.sort_by(|a, b| {
                (a.run.r#final, attention(&a.run.status))
                    .cmp(&(b.run.r#final, attention(&b.run.status)))
                    .then_with(|| {
                        words::parse_time(&b.run.started_at)
                            .cmp(&words::parse_time(&a.run.started_at))
                    })
                    .then_with(|| b.run.run_id.cmp(&a.run.run_id))
            });
            groups.push(Group {
                node: node.clone(),
                slug: mission.slug.clone(),
                title: if mission.title.is_empty() {
                    mission.slug.clone()
                } else {
                    mission.title.clone()
                },
                runs,
            });
        }
    }
    groups.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| (&a.node, &a.slug).cmp(&(&b.node, &b.slug)))
    });
    groups
}
pub fn matches(query: &str, row: &Row) -> bool {
    query.is_empty()
        || format!(
            "run {} {} {} {} {}",
            row.number,
            row.run.run_id,
            row.about,
            row.run.status,
            words::state_word(&row.run.status)
        )
        .to_lowercase()
        .contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Feed;
    use serde_json::json;
    fn run(id: &str, when: &str, status: &str, final_: bool) -> RunProjection {
        serde_json::from_value(json!({"runId":id,"status":status,"final":final_,"resumeAllowed":false,"epoch":1,"missionSlug":"demo","missionId":"m","missionRev":1,"inputCardId":"i","work":"","cwd":"","repository":"","base":"","contextPaths":[],"inputs":[],"outputs":[],"eventCount":1,"startedBy":"","startedAt":when,"updatedAt":when,"waitingGates":[],"events":[]})).unwrap()
    }
    #[test]
    fn groups_keep_run_numbers_in_time_order_but_put_attention_first() {
        let mut store = Store::default();
        let runs = [
            run("run_a", "2026-01-01T00:00:00Z", "succeeded", true),
            run("run_b", "2026-01-02T00:00:00Z", "blocked", false),
            run("run_c", "2026-01-03T00:00:00Z", "waiting_human", false),
        ];
        store.nodes.insert(
            "node".into(),
            Feed {
                missions: Some(vec![
                    serde_json::from_value(json!({"id":"m","slug":"demo","title":"Demo","rev":1}))
                        .unwrap(),
                ]),
                runs: runs
                    .into_iter()
                    .map(|run| (run.run_id.clone(), run))
                    .collect(),
            },
        );
        let groups = groups(&store);
        let rows = &groups[0].runs;
        assert_eq!(
            rows.iter()
                .map(|r| (r.run.run_id.as_str(), r.number))
                .collect::<Vec<_>>(),
            [("run_c", 3), ("run_b", 2), ("run_a", 1)]
        );
        assert!(matches("run 2", &rows[1]));
        assert!(!matches("run 2", &rows[0]));
    }
}
