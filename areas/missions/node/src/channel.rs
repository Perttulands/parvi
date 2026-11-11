//! Subscription to node-local mission changes; no timer is needed.
use super::Missions;
use parvi_missions_contract::{RUNS_SNAPSHOT, SNAPSHOT, WATCH, WATCH_RUNS};
use parvi_node_kit::{BoxFuture, Channel};
use std::collections::HashMap;
use tokio::sync::broadcast;
pub struct Connection {
    missions: Missions,
    watching: Option<broadcast::Receiver<()>>,
    initial: bool,
    runs: Option<broadcast::Receiver<parvi_missions_contract::RunProjection>>,
    runs_initial: bool,
    run_versions: HashMap<String, usize>,
}

impl Connection {
    pub fn new(missions: Missions) -> Self {
        Self {
            missions,
            watching: None,
            initial: false,
            runs: None,
            runs_initial: false,
            run_versions: HashMap::new(),
        }
    }
    pub fn receive(&mut self, frame: &[u8]) -> anyhow::Result<()> {
        match frame {
            [WATCH_RUNS, 0] => {
                self.runs = None;
                self.runs_initial = false;
                self.run_versions.clear();
            }
            [WATCH_RUNS, _] => {
                if self.runs.is_none() {
                    self.runs = Some(self.missions.coordinator.subscribe());
                    self.runs_initial = true;
                }
            }
            [WATCH, 0] => {
                self.watching = None;
                self.initial = false;
            }
            [WATCH, _] => {
                if self.watching.is_none() {
                    self.watching = Some(self.missions.changes.subscribe());
                    self.initial = true;
                }
            }
            _ => anyhow::bail!("watch-missions frame needs one byte"),
        }
        Ok(())
    }
    pub async fn next(&mut self) -> Vec<u8> {
        if self.runs_initial {
            self.runs_initial = false;
            return self.runs_snapshot();
        }
        while !self.initial {
            let missions = async {
                match &mut self.watching {
                    Some(rx) => {
                        let _ = rx.recv().await;
                    }
                    None => std::future::pending().await,
                }
            };
            let runs = async {
                match &mut self.runs {
                    Some(rx) => rx.recv().await.ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = missions => break,
                projection = runs => {
                    if let Some(projection) = projection {
                        let version = self.run_versions.entry(projection.run_id.clone()).or_default();
                        if projection.event_count <= *version {
                            continue;
                        }
                        *version = projection.event_count;
                        let mut frame = vec![RUNS_SNAPSHOT];
                        serde_json::to_writer(&mut frame, &vec![projection]).expect("run projection serializes");
                        return frame;
                    }
                    return self.runs_snapshot();
                },
            }
        }
        self.initial = false;
        let mut frame = vec![SNAPSHOT];
        let data = self.missions.store.list_missions().unwrap_or_default();
        serde_json::to_writer(&mut frame, &serde_json::json!({"missions": data}))
            .expect("mission summaries serialize");
        frame
    }
    fn runs_snapshot(&mut self) -> Vec<u8> {
        let mut frame = vec![RUNS_SNAPSHOT];
        let data: Vec<_> = self
            .missions
            .coordinator
            .list(None, false)
            .ok()
            .map(|runs| {
                runs.into_iter()
                    .filter_map(|run| self.missions.coordinator.run(&run.run_id).ok())
                    .collect()
            })
            .unwrap_or_default();
        for projection in &data {
            let version = self
                .run_versions
                .entry(projection.run_id.clone())
                .or_default();
            *version = (*version).max(projection.event_count);
        }
        serde_json::to_writer(&mut frame, &data).expect("run projections serialize");
        frame
    }
}

impl Channel for Connection {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = Connection::receive(self, &frame);
        })
    }
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move { Some(Connection::next(self).await) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_mission_engine::{Coordinator, LabExecutor, RunSettings, Store};
    struct NoResolver;
    impl parvi_work_contract::Resolver for NoResolver {
        fn resolve<'a>(&'a self, _: &'a str) -> parvi_work_contract::Resolving<'a> {
            Box::pin(async { Err(parvi_core::Error::new("bad_request", "unused")) })
        }
    }
    #[tokio::test]
    async fn watches_missions_and_unsubscribes() {
        let root = parvi_testkit::fresh_dir("missions-channel");
        let coordinator =
            Coordinator::open(&root, RunSettings::default(), Box::new(LabExecutor)).unwrap();
        let missions = Missions {
            store: Store::new(&root),
            coordinator,
            notify_task: None,
            changes: broadcast::channel(32).0,
            resolver: std::sync::Arc::new(NoResolver),
            node: "test".into(),
        };
        let mut socket = Connection::new(missions.clone());
        socket.receive(&[WATCH, 1]).unwrap();
        let first = socket.next().await;
        assert_eq!(first[0], SNAPSHOT);
        let value: parvi_missions_contract::MissionSnapshot =
            serde_json::from_slice(&first[1..]).unwrap();
        assert!(value.missions.is_empty());
        missions
            .store
            .create_mission("example", "Example", "human:test")
            .unwrap();
        missions.changed();
        let next = socket.next().await;
        let value: parvi_missions_contract::MissionSnapshot =
            serde_json::from_slice(&next[1..]).unwrap();
        assert_eq!(value.missions[0].slug, "example");
        socket.receive(&[WATCH, 0]).unwrap();
        assert!(socket.watching.is_none());
        drop(socket);
        drop(missions);
        std::fs::remove_dir_all(root).unwrap();
    }
    async fn snapshot_discards_older_updates(lagged: bool) {
        use parvi_mission_engine::{Ledger, RunEvent, project_run};
        let root = parvi_testkit::fresh_dir("missions-channel-order");
        let coordinator =
            Coordinator::open(&root, RunSettings::default(), Box::new(LabExecutor)).unwrap();
        let missions = Missions {
            store: Store::new(&root),
            coordinator,
            notify_task: None,
            changes: broadcast::channel(32).0,
            resolver: std::sync::Arc::new(NoResolver),
            node: "test".into(),
        };
        let ledger = Ledger::new(root.join("runs/run_test.ndjson"), "run_test");
        let event = |kind: &str| RunEvent {
            kind: kind.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            actor: "test".into(),
            mission_id: "mission".into(),
            ..Default::default()
        };
        ledger.create(event("run_started")).unwrap();
        let projection = || project_run(&ledger.read().unwrap(), "run_test").unwrap();
        let old = projection();
        let (tx, rx) = broadcast::channel(2);
        let mut channel = Connection::new(missions);
        channel.runs = Some(rx);
        channel.runs_initial = !lagged;
        tx.send(old.clone()).unwrap();
        if lagged {
            tx.send(old.clone()).unwrap();
            tx.send(old).unwrap();
        }
        ledger.append(event("node_started")).unwrap();
        let frame = channel.next().await;
        assert_eq!(frame[0], RUNS_SNAPSHOT);
        let snapshot: serde_json::Value = serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(snapshot[0]["eventCount"], 2);
        ledger.append(event("run_succeeded")).unwrap();
        tx.send(projection()).unwrap();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), channel.next())
            .await
            .unwrap();
        let update: serde_json::Value = serde_json::from_slice(&frame[1..]).unwrap();
        assert_eq!(update[0]["eventCount"], 3);
        assert_eq!(update[0]["status"], "succeeded");
        drop(channel);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn initial_snapshot_does_not_regress_to_queued_updates() {
        snapshot_discards_older_updates(false).await;
    }
    #[tokio::test]
    async fn lag_snapshot_does_not_regress_to_queued_updates() {
        snapshot_discards_older_updates(true).await;
    }
}
