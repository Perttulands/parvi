//! CHROTE's scheduler and API tests (`scheduled/scheduler_test.go`,
//! `api/scheduled_test.go`), in Rust, against a fake runner.

use super::*;
use axum::body::Body;
use axum::http::Request;
use parvi_scheduled_contract::{ERROR, PARTIAL, SUBMIT_KEY_DISPATCHED, SUCCESS};
use tower::ServiceExt;

const T0: &str = "2026-06-27T14:00:00Z";

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn target(session: &str) -> Target {
    Target {
        session_name: session.into(),
        unix_user: "alice".into(),
    }
}

/// Knows the sessions it was told about and records what it sent.
#[derive(Default)]
struct Fake {
    allowed: Mutex<HashSet<Target>>,
    sent: Mutex<Vec<(Target, String)>>,
    /// Sessions that refuse the paste.
    dead: HashSet<String>,
    /// Holds a send until released, and says it has begun.
    gate: Option<(tokio::sync::mpsc::UnboundedSender<()>, Arc<Notify>)>,
    validate_delay: Duration,
    /// Calls in flight now, and the most there ever were at once.
    busy: Mutex<(usize, usize)>,
}

impl Fake {
    /// Holds one call open briefly and counts how many overlap.
    async fn overlap(&self) {
        {
            let mut b = self.busy.lock().unwrap();
            b.0 += 1;
            b.1 = b.1.max(b.0);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.busy.lock().unwrap().0 -= 1;
    }
    fn most_at_once(&self) -> usize {
        self.busy.lock().unwrap().1
    }
}

impl Fake {
    fn allow(self, sessions: &[&str]) -> Fake {
        self.allowed
            .lock()
            .unwrap()
            .extend(sessions.iter().map(|s| target(s)));
        self
    }
    fn sends(&self) -> usize {
        self.sent.lock().unwrap().len()
    }
}

impl Runner for Fake {
    fn validate<'a>(&'a self, t: &'a Target) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.overlap().await;
            tokio::time::sleep(self.validate_delay).await;
            if t.unix_user == "intruder" {
                return Err(format!("unix user {:?} is not allowed", t.unix_user));
            }
            if self.allowed.lock().unwrap().contains(t) {
                Ok(())
            } else {
                Err(format!("no session {}", t.label()))
            }
        })
    }

    fn send<'a>(
        &'a self,
        t: &'a Target,
        prompt: &'a str,
        _deadline: Duration,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.overlap().await;
            self.sent.lock().unwrap().push((t.clone(), prompt.into()));
            if let Some((entered, release)) = &self.gate {
                let _ = entered.send(());
                release.notified().await;
            }
            if self.dead.contains(&t.session_name) {
                return Err("session is gone".into());
            }
            Ok("%7".into())
        })
    }
}

fn service(dir: &std::path::Path, fake: Arc<Fake>, now: Arc<Mutex<Timestamp>>) -> Scheduled {
    let mut s = Scheduled::new(dir.to_path_buf(), fake);
    s.now = Box::new(move || *now.lock().unwrap());
    s
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("parvi-scheduled-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn persisted(id: &str, every: i64, next: Option<&str>) -> Task {
    Task {
        id: id.into(),
        name: "persisted".into(),
        prompt: "hello from scheduler".into(),
        targets: vec![target("ops")],
        schedule: Schedule {
            kind: "interval".into(),
            every_minutes: every,
            timezone: "UTC".into(),
            ..Schedule::default()
        },
        enabled: true,
        paused: false,
        next_run: next.map(at),
        last_run: None,
        last_status: String::new(),
        created_by: String::new(),
        updated_by: String::new(),
        created_at: at("2026-06-27T13:00:00Z"),
        updated_at: at("2026-06-27T13:00:00Z"),
        recent_runs: vec![],
        audit: vec![],
        held: String::new(),
        attempt: None,
    }
}

/// TestSchedulerTickReloadsTasksComputesNextRunAndFiresDueTasks.
#[tokio::test]
async fn the_timer_computes_next_runs_and_fires_due_tasks_once() {
    let dir = scratch("tick");
    let now = Arc::new(Mutex::new(at(T0)));
    let fake = Arc::new(Fake::default().allow(&["ops"]));
    let s = service(&dir, fake.clone(), now.clone());
    s.store.save(&persisted("tsk_manual", 10, None)).unwrap();
    s.prime().unwrap();
    let want = at("2026-06-27T14:10:00Z");
    assert_eq!(s.get("tsk_manual").unwrap().next_run, Some(want));
    assert!(s.run_due().await.unwrap().is_empty());

    *now.lock().unwrap() = at("2026-06-27T14:05:00Z");
    assert!(s.run_due().await.unwrap().is_empty(), "not due yet");
    assert_eq!(
        s.get("tsk_manual").unwrap().next_run,
        Some(want),
        "no drift"
    );
    assert_eq!(fake.sends(), 0);

    *now.lock().unwrap() = want;
    assert_eq!(s.run_due().await.unwrap().len(), 1);
    let fired = s.get("tsk_manual").unwrap();
    assert_eq!(fake.sent.lock().unwrap()[0].1, "hello from scheduler");
    assert_eq!(fired.last_status, SUCCESS);
    assert_eq!(fired.recent_runs.len(), 1);
    assert_eq!(fired.next_run, Some(at("2026-06-27T14:20:00Z")));
    assert!(s.run_due().await.unwrap().is_empty(), "fires once");
    std::fs::remove_dir_all(dir).unwrap();
}

/// TestSchedulerDoesNotResurrectTaskDeletedDuringFire.
#[tokio::test]
async fn a_task_deleted_while_it_fires_stays_deleted() {
    let dir = scratch("resurrect");
    let now = Arc::new(Mutex::new(at(T0)));
    let release = Arc::new(Notify::new());
    let (entered, mut began) = tokio::sync::mpsc::unbounded_channel();
    let fake = Arc::new(
        Fake {
            gate: Some((entered, release.clone())),
            ..Fake::default()
        }
        .allow(&["ops"]),
    );
    let s = Arc::new(service(&dir, fake, now));
    s.store
        .save(&persisted("tsk_gone", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    s.prime().unwrap();
    let firing = tokio::spawn({
        let s = s.clone();
        async move { s.run_due().await }
    });
    began.recv().await.unwrap();
    assert!(
        s.store.delete("tsk_gone").unwrap(),
        "deleted by hand mid-fire"
    );
    release.notify_one();
    firing.await.unwrap().unwrap();
    assert!(matches!(s.get("tsk_gone"), Err(Error::NotFound)));
    std::fs::remove_dir_all(dir).unwrap();
}

/// TestFireTaskFansOutToEveryTargetAndSurvivesOneDeadSession.
#[tokio::test]
async fn a_run_fans_out_and_survives_one_dead_session() {
    let dir = scratch("fanout");
    let now = Arc::new(Mutex::new(at(T0)));
    let fake = Arc::new(
        Fake {
            dead: ["worker-2".to_string()].into(),
            ..Fake::default()
        }
        .allow(&["worker-1", "worker-2", "worker-3"]),
    );
    let s = service(&dir, fake.clone(), now);
    let mut task = persisted("tsk_fan", 10, None);
    task.targets = vec![target("worker-1"), target("worker-2"), target("worker-3")];
    let run = s.fire(&mut task, "manual").await.unwrap();
    assert_eq!(fake.sends(), 3, "a dead session does not stop the rest");
    assert_eq!(run.status, PARTIAL);
    assert_eq!(run.message, "alice/worker-2: session is gone");
    let order: Vec<_> = run
        .targets
        .iter()
        .map(|t| t.session_name.as_str())
        .collect();
    assert_eq!(
        order,
        ["worker-1", "worker-2", "worker-3"],
        "in the task's order"
    );
    assert!(run.targets[0].submit_key_dispatched);
    assert_eq!(run.targets[0].message, SUBMIT_KEY_DISPATCHED);
    assert_eq!(run.targets[1].status, ERROR);
    assert_eq!(task.last_status, PARTIAL);
    let _ = std::fs::remove_dir_all(dir);
}

/// TestFireTaskBoundsConcurrentFanOutAndPreservesTargetOrder and
/// TestCreateBoundsConcurrentTargetValidation.
#[tokio::test]
async fn checks_and_sends_overlap_at_most_eight_at_a_time() {
    let dir = scratch("bounded");
    let names: Vec<String> = (0..32).map(|i| format!("worker-{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let fake = Arc::new(Fake::default().allow(&refs));
    let s = service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0))));
    let mut task = persisted("tsk_wide", 10, None);
    task.targets = names.iter().map(|n| target(n)).collect();
    s.prepare(&mut task, true, at(T0)).await.unwrap();
    assert_eq!(fake.most_at_once(), CONCURRENT_DELIVERIES, "validation");
    *fake.busy.lock().unwrap() = (0, 0);
    let run = s.fire(&mut task, "manual").await.unwrap();
    assert_eq!(run.status, SUCCESS);
    assert_eq!(fake.most_at_once(), CONCURRENT_DELIVERIES, "delivery");
    let order: Vec<_> = run.targets.iter().map(|t| &t.session_name).collect();
    assert_eq!(
        order,
        names.iter().collect::<Vec<_>>(),
        "in the task's order"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// TestConcurrentRunNowCannotDispatchTwiceOrLoseReceipt,
/// TestRunNowBlocksConcurrentTaskMutation and
/// TestManualRunClaimPreventsConcurrentScheduledDispatch.
#[tokio::test]
async fn a_running_task_refuses_every_other_mutation_and_the_timer() {
    let dir = scratch("claims");
    let now = Arc::new(Mutex::new(at(T0)));
    let release = Arc::new(Notify::new());
    let (entered, mut began) = tokio::sync::mpsc::unbounded_channel();
    let fake = Arc::new(
        Fake {
            gate: Some((entered, release.clone())),
            ..Fake::default()
        }
        .allow(&["ops"]),
    );
    let s = Arc::new(service(&dir, fake.clone(), now));
    s.store
        .save(&persisted("tsk_busy", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    s.prime().unwrap();
    let manual = tokio::spawn({
        let s = s.clone();
        async move { s.run_now("tsk_busy", "first").await.map(|_| ()) }
    });
    began.recv().await.unwrap();
    let conflict = |r: Result<()>| matches!(r, Err(Error::Conflict(_)));
    assert!(conflict(s.run_now("tsk_busy", "second").await.map(|_| ())));
    let rename = Patch {
        name: Some("stale patch".into()),
        ..Patch::default()
    };
    assert!(conflict(s.patch("tsk_busy", rename).await.map(|_| ())));
    assert!(conflict(s.pause("tsk_busy", "pauser").map(|_| ())));
    assert!(conflict(s.resume("tsk_busy", "resumer").await.map(|_| ())));
    assert!(conflict(s.delete("tsk_busy")));
    assert!(s.run_due().await.unwrap().is_empty(), "the timer leaves it");
    assert!(
        Error::Conflict("tsk_busy".into())
            .to_string()
            .contains("already being modified")
    );
    release.notify_one();
    manual.await.unwrap().unwrap();
    let task = s.get("tsk_busy").unwrap();
    assert_eq!((task.recent_runs.len(), fake.sends()), (1, 1));
    std::fs::remove_dir_all(dir).unwrap();
}

/// TestDeliveryRunTimeoutIncludesValidation.
#[tokio::test]
async fn one_deadline_covers_validation_and_the_paste() {
    let dir = scratch("deadline");
    let fake = Arc::new(
        Fake {
            validate_delay: Duration::from_millis(60),
            ..Fake::default()
        }
        .allow(&["worker-1"]),
    );
    let mut s = service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0))));
    s.run_timeout = Duration::from_millis(20);
    let mut task = persisted("tsk_budget", 10, None);
    task.targets = vec![target("worker-1")];
    let run = s.fire(&mut task, "manual").await.unwrap();
    assert_eq!(fake.sends(), 0, "no fresh deadline after validation");
    assert_eq!(run.status, ERROR);
    assert!(run.targets[0].message.contains("deadline"), "{run:?}");
}

#[tokio::test]
async fn the_timer_sleeps_to_the_earliest_task_and_wakes_on_a_change() {
    let dir = scratch("timer");
    let fake = Arc::new(Fake::default().allow(&["ops"]));
    let s = Arc::new(Scheduled::new(dir.clone(), fake.clone()));
    let timer = tokio::spawn(s.clone().run());
    let soon = jiff::Timestamp::now() + jiff::SignedDuration::from_millis(150);
    let mut task = persisted("tsk_soon", 10, None);
    task.next_run = Some(soon);
    s.save(&task).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(fake.sends(), 1, "fired at its time, woken by the save");
    assert!(s.get("tsk_soon").unwrap().next_run.unwrap() > soon);
    timer.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 13: a run whose record would not save kept its past due time,
/// so the timer sent the same prompt again and again.
#[cfg(unix)]
#[tokio::test]
async fn a_run_that_cannot_be_recorded_is_held_back_not_sent_again() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("unsaved");
    let release = Arc::new(Notify::new());
    let (entered, mut began) = tokio::sync::mpsc::unbounded_channel();
    let fake = Arc::new(
        Fake {
            gate: Some((entered, release.clone())),
            ..Fake::default()
        }
        .allow(&["ops"]),
    );
    let s = Arc::new(service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0)))));
    s.store
        .save(&persisted("tsk_full", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    s.prime().unwrap();
    let firing = tokio::spawn({
        let s = s.clone();
        async move { s.run_due().await }
    });
    began.recv().await.unwrap();
    // The disk fills (here: the folder turns read-only) while it sends.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    release.notify_one();
    assert!(firing.await.unwrap().is_err(), "said, not swallowed");
    assert_eq!(fake.sends(), 1);
    assert!(s.run_due().await.unwrap().is_empty(), "never sent again");
    assert_eq!(fake.sends(), 1);
    let held = s.get("tsk_full").unwrap().held;
    assert!(held.contains("could not be saved"), "{held}");
    let (events, _) = s.events.listen();
    assert!(
        events
            .iter()
            .any(|e| e.failure && e.text.contains("tsk_full"))
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Resumed, it saves again and is no longer held.
    s.resume("tsk_full", "op").await.unwrap();
    assert!(s.get("tsk_full").unwrap().held.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 13: a task file that stopped reading was read again on every
/// wake, making no progress.
#[tokio::test]
async fn a_due_task_whose_file_does_not_read_is_held_back() {
    let dir = scratch("unreadable");
    let fake = Arc::new(Fake::default().allow(&["ops"]));
    let s = service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0))));
    s.save(&persisted("tsk_torn", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    std::fs::write(dir.join("tsk_torn.json"), "{ torn").unwrap();
    assert!(s.run_due().await.is_err());
    assert!(
        !s.due.lock().unwrap().contains_key("tsk_torn"),
        "not tried again"
    );
    assert!(s.run_due().await.unwrap().is_empty());
    assert_eq!(fake.sends(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 17: a node stopped mid-send left the task due at the time it
/// was sending for, so the restarted node sent the same prompt again.
#[tokio::test]
async fn a_run_cut_short_by_a_restart_is_held_unconfirmed_not_sent_again() {
    let dir = scratch("restart");
    let release = Arc::new(Notify::new());
    let (entered, mut began) = tokio::sync::mpsc::unbounded_channel();
    let fake = Arc::new(
        Fake {
            gate: Some((entered, release.clone())),
            ..Fake::default()
        }
        .allow(&["ops"]),
    );
    let now = Arc::new(Mutex::new(at(T0)));
    let first = Arc::new(service(&dir, fake.clone(), now.clone()));
    first
        .store
        .save(&persisted("tsk_cut", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    first.prime().unwrap();
    let firing = tokio::spawn({
        let s = first.clone();
        async move { s.run_due().await }
    });
    began.recv().await.unwrap();
    // The node stops while the prompt is going out.
    firing.abort();
    let _ = firing.await;
    drop(first);
    assert_eq!(fake.sends(), 1);

    let again = Arc::new(Fake::default().allow(&["ops"]));
    let s = service(&dir, again.clone(), now.clone());
    s.prime().unwrap();
    assert!(s.run_due().await.unwrap().is_empty());
    *now.lock().unwrap() = at("2026-06-27T15:00:00Z");
    assert!(
        s.run_due().await.unwrap().is_empty(),
        "not at its next time either"
    );
    assert_eq!(again.sends(), 0, "never sent again");
    let held = s.get("tsk_cut").unwrap().held;
    assert!(
        held.contains("unconfirmed") && held.contains("check the agent"),
        "{held}"
    );
    assert!(s.list().unwrap()[0].held.contains("unconfirmed"));

    // The operator checks the agent and resumes it: it runs again.
    s.resume("tsk_cut", "op").await.unwrap();
    let task = s.get("tsk_cut").unwrap();
    assert!(task.held.is_empty() && task.attempt.is_none());
    *now.lock().unwrap() = task.next_run.unwrap();
    assert_eq!(s.run_due().await.unwrap().len(), 1);
    assert_eq!(again.sends(), 1);
    assert!(
        s.get("tsk_cut").unwrap().attempt.is_none(),
        "recorded, settled"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 17: a run that cannot be written ahead is not sent at all,
/// scheduled or manual.
#[cfg(unix)]
#[tokio::test]
async fn a_run_that_cannot_be_written_ahead_sends_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("ahead");
    let fake = Arc::new(Fake::default().allow(&["ops"]));
    let s = service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0))));
    s.save(&persisted("tsk_ro", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(s.run_due().await.is_err(), "said, not swallowed");
    assert!(s.run_now("tsk_ro", "op").await.is_err());
    assert!(s.run_due().await.unwrap().is_empty());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(fake.sends(), 0);
    let held = s.get("tsk_ro").unwrap().held;
    assert!(held.contains("nothing was sent"), "{held}");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 17: a manual run whose record would not save left the task as
/// it was, due, so the timer sent it again.
#[cfg(unix)]
#[tokio::test]
async fn a_manual_run_that_cannot_be_recorded_is_held_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("manual-unsaved");
    let release = Arc::new(Notify::new());
    let (entered, mut began) = tokio::sync::mpsc::unbounded_channel();
    let fake = Arc::new(
        Fake {
            gate: Some((entered, release.clone())),
            ..Fake::default()
        }
        .allow(&["ops"]),
    );
    let s = Arc::new(service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0)))));
    s.save(&persisted("tsk_hand", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    let manual = tokio::spawn({
        let s = s.clone();
        async move { s.run_now("tsk_hand", "op").await }
    });
    began.recv().await.unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    release.notify_one();
    assert!(manual.await.unwrap().is_err());
    assert!(s.run_due().await.unwrap().is_empty(), "not sent again");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(fake.sends(), 1);
    let held = s.get("tsk_hand").unwrap().held;
    assert!(
        held.contains("manual run") && held.contains("could not be saved"),
        "{held}"
    );
    // A restart does not resend it either: its attempt was written ahead.
    let again = Arc::new(Fake::default().allow(&["ops"]));
    let restarted = service(&dir, again.clone(), Arc::new(Mutex::new(at(T0))));
    restarted.prime().unwrap();
    assert!(restarted.run_due().await.unwrap().is_empty());
    assert_eq!(again.sends(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 17: one task file that did not read failed the whole list and
/// stopped the node from starting.
#[tokio::test]
async fn one_unreadable_file_neither_hides_the_rest_nor_stops_the_node() {
    let dir = scratch("broken");
    let fake = Arc::new(Fake::default().allow(&["ops"]));
    let s = service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0))));
    s.store
        .save(&persisted("tsk_good", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    std::fs::write(dir.join("tsk_bad.json"), "{ torn").unwrap();
    s.prime().expect("the node starts");
    let tasks = s.list().unwrap();
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    assert_eq!(tasks[0].id, "tsk_good");
    assert!(tasks[0].held.is_empty());
    assert_eq!(tasks[1].id, "tsk_bad");
    assert!(tasks[1].held.contains("does not read"), "{}", tasks[1].held);
    assert!(!tasks[1].enabled && tasks[1].next_run.is_none());
    let runs = s.run_due().await.unwrap();
    assert_eq!(runs.len(), 1, "the healthy task still runs");
    assert_eq!(fake.sends(), 1);
    // Over HTTP too.
    let api = Api {
        app: crate::routes::routes(Arc::new(s)),
        fake,
        dir: dir.clone(),
    };
    let listed = api.ok("GET", "/api/scheduled-tasks", "").await;
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 2, "{listed}");
    assert_eq!(listed["tasks"][1]["id"], "tsk_bad");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Review 13: a due task held by a manual run made the timer retry at
/// once, over and over, until the claim went.
#[tokio::test]
async fn a_claimed_due_task_waits_for_its_claim_without_spinning() {
    let dir = scratch("claimed");
    let fake = Arc::new(Fake::default().allow(&["ops"]));
    let looks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut s = Scheduled::new(dir.clone(), fake.clone());
    s.now = Box::new({
        let looks = looks.clone();
        move || {
            looks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            at(T0)
        }
    });
    let s = Arc::new(s);
    s.save(&persisted("tsk_busy", 10, Some("2026-06-27T13:59:00Z")))
        .unwrap();
    let claim = s.claim("tsk_busy").unwrap();
    let timer = tokio::spawn(s.clone().run());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.sends(), 0, "left to its claim");
    let idle = looks.load(std::sync::atomic::Ordering::SeqCst);
    assert!(idle <= 2, "the timer waits, it does not spin: {idle} looks");
    drop(claim);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.sends(), 1, "the claim's release woke it");
    timer.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

// The API, as CHROTE's api/scheduled_test.go drives it.

struct Api {
    app: Router,
    fake: Arc<Fake>,
    dir: PathBuf,
}

use axum::Router;

fn api(name: &str, sessions: &[&str]) -> Api {
    let dir = scratch(name);
    let fake = Arc::new(Fake::default().allow(sessions));
    let s = Arc::new(service(&dir, fake.clone(), Arc::new(Mutex::new(at(T0)))));
    Api {
        app: crate::routes::routes(s),
        fake,
        dir,
    }
}

impl Api {
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: &str,
        intent: bool,
        json: bool,
    ) -> (u16, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if intent {
            req = req.header("X-Chrote-Intent", "scheduled-task");
        }
        req = req.header(
            "Content-Type",
            if json {
                "application/json"
            } else {
                "text/plain"
            },
        );
        let res = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn ok(&self, method: &str, path: &str, body: &str) -> Value {
        let (status, v) = self.call(method, path, body, true, true).await;
        assert_eq!(status, 200, "{method} {path}: {v}");
        assert_eq!(v["success"], true);
        assert!(v["timestamp"].is_string());
        v["data"].clone()
    }
}

use serde_json::Value;

/// TestScheduledTasksAPIEnvelopeAndLifecycle.
#[tokio::test]
async fn api_envelope_and_lifecycle() {
    let a = api("api-life", &["ops"]);
    let created = a
        .ok(
            "POST",
            "/api/scheduled-tasks",
            r#"{"name":"Standup nudge","prompt":"hello; rm -rf / $(whoami)",
                "targets":[{"sessionName":"ops","unixUser":"alice"}],
                "schedule":{"type":"interval","everyMinutes":15,"timezone":"UTC"},
                "createdBy":"agent:test"}"#,
        )
        .await;
    let task = &created["task"];
    let id = task["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("tsk_"));
    assert_eq!(task["enabled"], true);
    assert_eq!(task["paused"], false);
    assert_eq!(task["nextRun"], "2026-06-27T14:15:00Z");
    assert_eq!(
        (task["createdBy"].as_str(), task["updatedBy"].as_str()),
        (Some("agent:test"), Some("agent:test"))
    );

    let listed = a.ok("GET", "/api/scheduled-tasks", "").await;
    assert_eq!(listed["tasks"][0]["id"], id.as_str());
    let read = a.ok("GET", &format!("/api/scheduled-tasks/{id}"), "").await;
    assert_eq!(read["task"]["id"], id.as_str());

    let patched = a
        .ok(
            "PATCH",
            &format!("/api/scheduled-tasks/{id}"),
            r#"{"name":"Renamed nudge","prompt":"patched literal prompt",
                "schedule":{"type":"interval","everyMinutes":30,"timezone":"UTC"},"updatedBy":"agent:patch"}"#,
        )
        .await;
    assert_eq!(patched["task"]["name"], "Renamed nudge");
    assert_eq!(patched["task"]["schedule"]["everyMinutes"], 30);
    assert_eq!(patched["task"]["updatedBy"], "agent:patch");

    let paused = a
        .ok("POST", &format!("/api/scheduled-tasks/{id}/pause"), "{}")
        .await;
    assert_eq!(paused["task"]["paused"], true);
    assert!(paused["task"].get("nextRun").is_none());
    let resumed = a
        .ok("POST", &format!("/api/scheduled-tasks/{id}/resume"), "{}")
        .await;
    assert_eq!(resumed["task"]["paused"], false);
    assert!(resumed["task"]["nextRun"].is_string());

    let ran = a
        .ok("POST", &format!("/api/scheduled-tasks/{id}/run-now"), "{}")
        .await;
    assert!(ran["run"].is_object());
    assert_eq!(ran["task"]["lastStatus"], "success");
    assert_eq!(ran["task"]["recentRuns"].as_array().unwrap().len(), 1);
    assert_eq!(a.fake.sent.lock().unwrap()[0].1, "patched literal prompt");

    let deleted = a
        .ok("DELETE", &format!("/api/scheduled-tasks/{id}"), "")
        .await;
    assert_eq!(deleted["deleted"], id.as_str());
    let (status, _) = a
        .call(
            "GET",
            &format!("/api/scheduled-tasks/{id}"),
            "",
            false,
            false,
        )
        .await;
    assert_eq!(status, 404);
    std::fs::remove_dir_all(&a.dir).unwrap();
}

/// TestScheduledTasksAPIFansOneTaskOutToManySessions.
#[tokio::test]
async fn api_fans_one_task_out_to_many_sessions() {
    let a = api("api-fan", &["worker-1", "worker-2", "worker-3"]);
    let created = a
        .ok(
            "POST",
            "/api/scheduled-tasks",
            r#"{"name":"Continue work","prompt":"Continue if work is clear",
                "targets":[{"sessionName":"worker-1","unixUser":"alice"},{"sessionName":"worker-2","unixUser":"alice"},{"sessionName":"worker-3","unixUser":"alice"}],
                "schedule":{"type":"cron","expression":"0 16 * * *","timezone":"Europe/Helsinki"}}"#,
        )
        .await;
    let id = created["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["task"]["targets"].as_array().unwrap().len(), 3);
    let ran = a
        .ok("POST", &format!("/api/scheduled-tasks/{id}/run-now"), "{}")
        .await;
    assert_eq!(ran["task"]["lastStatus"], "success");
    assert_eq!(a.fake.sends(), 3);
    assert_eq!(
        ran["task"]["recentRuns"][0]["targets"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let patched = a
        .ok(
            "PATCH",
            &format!("/api/scheduled-tasks/{id}"),
            r#"{"targets":[{"sessionName":"worker-2","unixUser":"alice"}],"updatedBy":"agent:patch"}"#,
        )
        .await;
    assert_eq!(
        patched["task"]["targets"],
        serde_json::json!([{"sessionName":"worker-2","unixUser":"alice"}])
    );
    std::fs::remove_dir_all(&a.dir).unwrap();
}

/// TestScheduledTasksAPIRequiresMutationIntentAndJSON.
#[tokio::test]
async fn api_requires_mutation_intent_and_json() {
    let a = api("api-intent", &["ops"]);
    let body = r#"{"name":"csrf","prompt":"hello","targets":[{"sessionName":"ops","unixUser":"alice"}],"schedule":{"type":"interval","everyMinutes":15,"timezone":"UTC"}}"#;
    let (status, v) = a
        .call("POST", "/api/scheduled-tasks", body, false, true)
        .await;
    assert_eq!(status, 403);
    assert!(v.to_string().contains("X-Chrote-Intent"));
    let (status, v) = a
        .call("POST", "/api/scheduled-tasks", body, true, false)
        .await;
    assert_eq!(status, 415);
    assert!(v.to_string().contains("application/json"));
    assert_eq!(a.fake.sends(), 0);
    let _ = std::fs::remove_dir_all(&a.dir);
}

/// TestScheduledTasksAPIValidationRejectsUnsafeOrInvalidRequests.
#[tokio::test]
async fn api_validation_rejects_unsafe_or_invalid_requests() {
    let ops = r#""targets":[{"sessionName":"ops","unixUser":"alice"}]"#;
    let every = r#""schedule":{"type":"interval","everyMinutes":15,"timezone":"UTC"}"#;
    for (body, mention) in [
        (
            format!(r#"{{"name":"bad","prompt":"   ",{ops},{every}}}"#),
            "",
        ),
        (
            format!(r#"{{"name":"no targets","prompt":"hello","targets":[],{every}}}"#),
            "at least one target",
        ),
        (
            format!(
                r#"{{"name":"bad","prompt":"hello",{ops},"schedule":{{"type":"interval","everyMinutes":0,"timezone":"UTC"}}}}"#
            ),
            "",
        ),
        (
            format!(
                r#"{{"name":"bad","prompt":"hello",{ops},"schedule":{{"type":"cron","expression":"61 * * * *","timezone":"UTC"}}}}"#
            ),
            "",
        ),
        (
            format!(
                r#"{{"name":"bad","prompt":"hello","targets":[{{"sessionName":"ops","unixUser":"alice","socket":"/tmp/evil.sock"}}],{every}}}"#
            ),
            "socket",
        ),
        (
            format!(
                r#"{{"name":"bad","prompt":"hello","targets":[{{"sessionName":"missing","unixUser":"alice"}}],{every}}}"#
            ),
            "target",
        ),
        (
            format!(
                r#"{{"name":"bad","prompt":"hello","targets":[{{"sessionName":"ops","unixUser":"intruder"}}],{every}}}"#
            ),
            "not allowed",
        ),
        (
            format!(
                r#"{{"name":"bad","prompt":"hello","target":{{"sessionName":"ops"}},{every}}}"#
            ),
            "retired",
        ),
    ] {
        let a = api("api-invalid", &["ops"]);
        let (status, v) = a
            .call("POST", "/api/scheduled-tasks", &body, true, true)
            .await;
        assert_eq!(status, 400, "{body}: {v}");
        assert_eq!(v["success"], false);
        assert!(v["error"]["message"].is_string() && v["timestamp"].is_string());
        assert!(v.to_string().contains(mention), "{body}: {v}");
        assert_eq!(a.fake.sends(), 0, "a refused task sent nothing");
        let _ = std::fs::remove_dir_all(&a.dir);
    }
}

/// TestWriteScheduledErrorReportsTaskMutationConflict.
#[test]
fn a_conflict_answers_409() {
    let r = crate::routes::failed(Error::Conflict("tsk_x".into()));
    assert_eq!(r.status().as_u16(), 409);
}

/// #237: a renamed session keeps its tasks, and only the server it is on
/// is followed.
#[tokio::test]
async fn a_renamed_session_keeps_its_tasks() {
    let dir = scratch("rename");
    let now = Arc::new(Mutex::new(at(T0)));
    let s = service(&dir, Arc::new(Fake::default()), now);
    let mut both = persisted("tsk_both", 10, None);
    both.targets = vec![target("ops"), target("build")];
    let mut other = persisted("tsk_other", 10, None);
    other.targets = vec![Target {
        session_name: "ops".into(),
        unix_user: "bob".into(),
    }];
    s.store.save(&both).unwrap();
    s.store.save(&other).unwrap();
    assert_eq!(
        s.follow_rename("alice", "ops", "ops-2").await.unwrap(),
        ["tsk_both"]
    );
    let both = s.get("tsk_both").unwrap();
    let names: Vec<_> = both
        .targets
        .iter()
        .map(|t| t.session_name.as_str())
        .collect();
    assert_eq!(names, ["ops-2", "build"]);
    assert_eq!(
        both.audit.last().unwrap().message,
        "session ops renamed to ops-2"
    );
    assert_eq!(
        s.get("tsk_other").unwrap().targets[0].session_name,
        "ops",
        "bob's ops is another session"
    );
    let _ = std::fs::remove_dir_all(dir);
}
