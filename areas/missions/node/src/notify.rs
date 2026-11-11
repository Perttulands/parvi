//! Reconcile open asks with the sent receipt; one worker serializes delivery.
use parvi_mission_engine::Coordinator;
use parvi_node_kit::events::Log;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::broadcast, time::Instant};

const RETRY: Duration = Duration::from_secs(5 * 60);

pub fn start(
    c: Coordinator,
    root: PathBuf,
    command: Option<PathBuf>,
    log: Arc<Log>,
) -> tokio::task::JoinHandle<()> {
    let changes = c.subscribe();
    let old_final = c
        .list(None, false)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.r#final)
        .map(|r| r.run_id)
        .collect();
    tokio::spawn(run(c, root, command, log, changes, old_final))
}

async fn run(
    c: Coordinator,
    root: PathBuf,
    command: Option<PathBuf>,
    log: Arc<Log>,
    mut changes: broadcast::Receiver<parvi_missions_contract::RunProjection>,
    old_final: BTreeSet<String>,
) {
    let mut state = Dispatch {
        root,
        command,
        log,
        old_final,
        failures: BTreeMap::new(),
        logged: BTreeSet::new(),
        delivered: BTreeMap::new(),
    };
    state.scan(&c).await;
    loop {
        let next_retry = state.failures.values().copied().min();
        tokio::select! {
            event = changes.recv() => match event {
                Ok(run) => state.deliver(&c, &run.run_id, Instant::now()).await,
                Err(broadcast::error::RecvError::Lagged(_)) => state.scan(&c).await,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = async { if let Some(at) = next_retry { tokio::time::sleep_until(at).await } }, if next_retry.is_some() => state.scan(&c).await,
        }
    }
}

struct Dispatch {
    root: PathBuf,
    command: Option<PathBuf>,
    log: Arc<Log>,
    old_final: BTreeSet<String>,
    failures: BTreeMap<(String, u64), Instant>,
    logged: BTreeSet<(String, u64)>,
    delivered: BTreeMap<String, BTreeSet<u64>>,
}
impl Dispatch {
    async fn scan(&mut self, c: &Coordinator) {
        if let Ok(runs) = c.list(None, false) {
            for run in runs {
                self.deliver(c, &run.run_id, Instant::now()).await;
            }
        }
    }
    async fn deliver(&mut self, c: &Coordinator, id: &str, now: Instant) {
        let Ok(asks) = c.notifications(id) else {
            return;
        };
        self.failures.retain(|(run, seq), _| {
            run != id || asks.iter().any(|ask| ask["seq"].as_u64() == Some(*seq))
        });
        let path = self.root.join("runs").join(format!("{id}.needs-you.json"));
        let mut sent = match read_receipts(&path) {
            Ok(sent) => sent,
            Err(e) => {
                eprintln!("parvi: needs-you receipts {id}: {e}");
                if self.command.is_some() {
                    for ask in &asks {
                        if let Some(seq) = ask["seq"].as_u64() {
                            self.failures.insert((id.into(), seq), now + RETRY);
                        }
                    }
                }
                return;
            }
        };
        if let Some(delivered) = self.delivered.get(id) {
            sent.extend(delivered);
            if let Err(error) = write_receipts(&path, &sent) {
                eprintln!("parvi: needs-you receipt {id}: {error}");
                for ((run, _), at) in &mut self.failures {
                    if run == id {
                        *at = now + RETRY;
                    }
                }
                return;
            }
            self.delivered.remove(id);
            self.failures
                .retain(|(run, seq), _| run != id || !sent.contains(seq));
        }
        for ask in asks {
            let seq = ask["seq"].as_u64().unwrap_or(0);
            let key = (id.to_owned(), seq);
            let kind = ask["kind"].as_str().unwrap_or("");
            if kind == "final" && self.old_final.contains(id) {
                continue;
            }
            if (kind == "human_gate" || kind == "escalation") && self.logged.insert(key.clone()) {
                self.log.record(
                    "mission",
                    false,
                    format!(
                        "run needs you: {id}: {}",
                        ask["text"].as_str().unwrap_or("")
                    ),
                );
            }
            if sent.contains(&seq)
                || self.command.is_none()
                || self.failures.get(&key).is_some_and(|at| *at > now)
            {
                continue;
            }
            if let Err(error) = send(self.command.as_ref().unwrap(), &ask).await {
                eprintln!("parvi: needs-you {id} {seq}: {error}");
                self.failures.insert(key, now + RETRY);
                continue;
            }
            sent.insert(seq);
            if let Err(error) = write_receipts(&path, &sent) {
                eprintln!("parvi: needs-you receipt {id} {seq}: {error}");
                // Keep the success in memory even if disk is temporarily unavailable.
                self.delivered.entry(id.into()).or_default().insert(seq);
                self.failures.insert(key, now + RETRY);
            } else {
                self.failures.remove(&key);
            }
        }
    }
}
fn read_receipts(path: &Path) -> std::io::Result<BTreeSet<u64>> {
    match std::fs::read(path) {
        Ok(data) => serde_json::from_slice(&data).map_err(std::io::Error::other),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e),
    }
}
fn write_receipts(path: &Path, sent: &BTreeSet<u64>) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("needs-you.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    serde_json::to_writer(&mut f, sent).map_err(std::io::Error::other)?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    std::fs::rename(tmp, path)?;
    std::fs::File::open(path.parent().unwrap())?.sync_all()
}
async fn send(path: &Path, ask: &Value) -> std::io::Result<()> {
    let mut child = tokio::process::Command::new(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(Duration::from_secs(30), async {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("notification stdin unavailable"))?;
        stdin
            .write_all(&serde_json::to_vec(ask).map_err(std::io::Error::other)?)
            .await?;
        drop(stdin);
        child.wait().await
    })
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "notification command timed out",
        )
    })??;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "notification command exited {status}"
        )))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use parvi_mission_engine::{LabExecutor, Ledger, RunEvent, RunSettings};
    use std::os::unix::fs::PermissionsExt;

    fn fixture(root: &Path, id: &str, final_run: bool) {
        let ledger = Ledger::new(root.join("runs").join(format!("{id}.ndjson")), id);
        let event = |kind: &str| RunEvent {
            kind: kind.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            actor: "agent:test".into(),
            ..Default::default()
        };
        let mut start = event("run_started");
        start.data.insert("missionSlug".into(), "hello".into());
        ledger.create(start).unwrap();
        let mut evaluating = event("gate_evaluating");
        evaluating.gate_id = "review".into();
        ledger.append(evaluating).unwrap();
        let mut request = event("human_input_requested");
        request.gate_id = "review".into();
        request.node_id = "review".into();
        request.data = serde_json::json!({"inputRef":{"text":"work","fromNodeId":"draft"}})
            .as_object()
            .unwrap()
            .clone();
        ledger.append(request).unwrap();
        if final_run {
            ledger.append(event("run_canceled")).unwrap();
        }
        std::fs::write(root.join("runs").join(format!("{id}.snapshot.toml")), "schema = 1\nid = 'msn_hello'\nslug = 'hello'\ntitle = 'Hello'\nrev = 1\n[[gate]]\nid = 'review'\ntitle = 'Review'\nkinds = ['human']\ncriterion = 'Approve this?'\n").unwrap();
    }
    fn dispatcher(
        root: &Path,
        command: &Path,
        old_final: BTreeSet<String>,
        log: Arc<Log>,
    ) -> Dispatch {
        Dispatch {
            root: root.into(),
            command: Some(command.into()),
            log,
            old_final,
            failures: BTreeMap::new(),
            logged: BTreeSet::new(),
            delivered: BTreeMap::new(),
        }
    }
    #[tokio::test]
    async fn retries_on_schedule_and_receipts_survive_restart() {
        let root = parvi_testkit::fresh_dir("needs-you-retry");
        let coordinator =
            Coordinator::open(&root, RunSettings::default(), Box::new(LabExecutor)).unwrap();
        fixture(&root, "run_live", false);
        fixture(&root, "run_old", true);
        let command = root.join("notify");
        let marker = root.join("failed-once");
        let output = root.join("messages");
        std::fs::write(&command, format!("#!/bin/sh\nif [ ! -f '{}' ]; then touch '{}'; exit 1; fi\ncat >> '{}'\nprintf '\\n' >> '{}'\n", marker.display(), marker.display(), output.display(), output.display())).unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
        let log = Arc::new(Log::default());
        let mut d = dispatcher(
            &root,
            &command,
            BTreeSet::from(["run_old".into()]),
            log.clone(),
        );
        let now = Instant::now();
        d.deliver(&coordinator, "run_live", now).await;
        assert!(!output.exists());
        d.deliver(
            &coordinator,
            "run_live",
            now + RETRY - Duration::from_secs(1),
        )
        .await;
        assert!(!output.exists());
        d.deliver(&coordinator, "run_live", now + RETRY).await;
        let messages = std::fs::read_to_string(&output).unwrap();
        let payload: Value = serde_json::from_str(messages.trim()).unwrap();
        assert_eq!(payload["runId"], "run_live");
        assert_eq!(payload["missionSlug"], "hello");
        assert_eq!(payload["gateId"], "review");
        assert_eq!(payload["ask"], "Approve this?");
        assert_eq!(payload["kind"], "human_gate");
        assert!(
            payload["body"]
                .as_str()
                .unwrap()
                .contains("parvi gate approve run_live review --requested-seq 3")
        );
        assert!(
            log.listen()
                .0
                .iter()
                .any(|event| event.text.starts_with("run needs you:"))
        );
        drop(d);
        let mut restarted = dispatcher(&root, &command, BTreeSet::from(["run_old".into()]), log);
        restarted
            .deliver(&coordinator, "run_live", now + RETRY)
            .await;
        restarted
            .deliver(&coordinator, "run_old", now + RETRY)
            .await;
        assert_eq!(std::fs::read_to_string(&output).unwrap(), messages);
        drop(restarted);
        drop(coordinator);
        std::fs::remove_dir_all(root).unwrap();
    }
}
