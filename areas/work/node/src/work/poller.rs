//! Keeps the configured projects' snapshot current while anyone watches
//! it: one poller per node, started by the first watcher and stopped with
//! the last, so an unwatched Work tab costs nothing (AGENTS.md: timers run
//! only while something subscribes). A Beads store is read as soon as
//! watching starts and then again only when its Dolt manifest changed since
//! the last attempt, failed or not; a store without a manifest Parvi can
//! read is not read again until watching starts anew, and says so. GitHub
//! is read when watching starts (unless it was read less than [`EVERY`]
//! ago, across watches) and again only when a watcher asks for it with
//! [`Work::refresh`]: nothing rereads a remote store on a timer
//! (`/srv/AGENTS.md`, Background work).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::beads::{self, Bd};
use super::config::{Config, Project};
use super::github::Gh;
use super::item::Item;
use super::model::{ProjectView, RepoView, Snapshot, StoreView};

/// How often a watched store's manifest is looked at, and how recent a
/// GitHub read a new watcher takes as it is. A Beads store costs a file
/// read unless it changed.
pub const EVERY: Duration = Duration::from_secs(60);

/// The Work module's state on this node. Cheap to clone.
#[derive(Clone)]
pub struct Work(pub(super) Arc<Inner>);

pub(super) struct Inner {
    pub(super) config: Config,
    /// This node's id, which its stores' references name.
    pub(super) node: String,
    pub(super) gh: Gh,
    pub(super) bd: Bd,
    snapshot: watch::Sender<Arc<Snapshot>>,
    poll: Mutex<Poll>,
    /// Each store's Beads as last read, with the manifest they were read at.
    stores: Mutex<HashMap<String, Read>>,
    /// Each store's id prefix, once asked: it does not change.
    pub(super) prefixes: Mutex<HashMap<String, String>>,
    /// A watcher asked to read everything again now.
    refresh: tokio::sync::Notify,
}

struct Read {
    marker: Option<Vec<u8>>,
    items: Arc<Vec<Item>>,
}

#[derive(Default)]
struct Poll {
    watchers: usize,
    task: Option<JoinHandle<()>>,
    /// When GitHub was last read, so a watcher who comes back soon gets
    /// the repositories as they were without spending the rate limit again.
    last_github: Option<Instant>,
    /// A refresh was asked for and not yet done.
    refresh: bool,
}

impl Default for Work {
    /// No projects.
    fn default() -> Work {
        Work::new(Config::default(), "node", Gh::default(), Bd::default())
    }
}

impl Work {
    pub fn new(config: Config, node: impl Into<String>, gh: Gh, bd: Bd) -> Work {
        let snapshot = Snapshot {
            projects: config
                .projects
                .iter()
                .map(|project| match project {
                    Project::GitHub(repo) => ProjectView::Github(RepoView {
                        name: repo.name.clone(),
                        checkout: repo.checkout.clone(),
                        ..RepoView::default()
                    }),
                    Project::Beads(store) => ProjectView::Beads(StoreView {
                        folder: store.folder.clone(),
                        ..StoreView::default()
                    }),
                })
                .collect(),
        };
        Work(Arc::new(Inner {
            config,
            node: node.into(),
            gh,
            bd,
            snapshot: watch::Sender::new(Arc::new(snapshot)),
            poll: Mutex::default(),
            stores: Mutex::default(),
            prefixes: Mutex::default(),
            refresh: tokio::sync::Notify::new(),
        }))
    }

    /// Reads every project again now, GitHub too, for the watchers: what a
    /// Refresh press asks. Only while some watcher keeps the poller running.
    pub fn refresh(&self) {
        self.0.poll.lock().unwrap().refresh = true;
        self.0.refresh.notify_one();
    }

    /// Starts watching: the snapshot now, and again each time it changes.
    /// The poller runs while any subscription lives.
    pub fn subscribe(&self) -> Subscription {
        let mut changes = self.0.snapshot.subscribe();
        changes.mark_changed();
        let mut poll = self.0.poll.lock().unwrap();
        poll.watchers += 1;
        if poll.task.is_none() && !self.0.config.projects.is_empty() {
            poll.task = Some(tokio::spawn(rounds(self.0.clone())));
        }
        Subscription {
            work: self.0.clone(),
            changes,
        }
    }
}

impl Inner {
    /// The store's Beads: as last read if its manifest has not changed
    /// since, else read now, as always when it has no manifest. Also says
    /// whether this read them.
    pub(super) async fn store(&self, folder: &str) -> Result<(Arc<Vec<Item>>, bool)> {
        let marker = beads::marker(folder).ok();
        if let Some(read) = self.stores.lock().unwrap().get(folder)
            && marker.is_some()
            && read.marker == marker
        {
            return Ok((read.items.clone(), false));
        }
        let json = self.bd.list(folder).await?;
        let items = Arc::new(super::beads_parse::parse(&json, &self.node, folder)?);
        self.stores.lock().unwrap().insert(
            folder.to_owned(),
            Read {
                marker,
                items: items.clone(),
            },
        );
        Ok((items, true))
    }
}

/// One watcher. Dropping it stops the poller if it was the last.
pub struct Subscription {
    work: Arc<Inner>,
    changes: watch::Receiver<Arc<Snapshot>>,
}

impl Subscription {
    /// The snapshot, first as it is and then each time it changes.
    /// Cancel-safe.
    pub async fn next(&mut self) -> Arc<Snapshot> {
        // Never an error: the sender lives in `work`, which this holds.
        let _ = self.changes.changed().await;
        self.changes.borrow_and_update().clone()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut poll = self.work.poll.lock().unwrap();
        poll.watchers -= 1;
        if poll.watchers == 0
            && let Some(task) = poll.task.take()
        {
            // Kills a read in flight too: gh and bd are kill_on_drop.
            task.abort();
        }
    }
}

/// Looks at every project in turn, at once and then each [`EVERY`] or
/// when a refresh is asked, publishing each as it arrives, until aborted.
/// The stores' manifests are looked at every round; GitHub is read in the
/// first round (unless read within [`EVERY`]) and in a refresh's.
async fn rounds(work: Arc<Inner>) {
    // Each store's marker at this session's last attempt to read it.
    let mut attempted: HashMap<usize, Result<Vec<u8>, String>> = HashMap::new();
    let mut github = github_wait(&work).is_zero();
    loop {
        if std::mem::take(&mut work.poll.lock().unwrap().refresh) {
            github = true;
            // Every store is read again, one without a manifest too.
            attempted.clear();
        }
        for (index, project) in work.config.projects.iter().enumerate() {
            match project {
                Project::GitHub(_) if !github => {}
                Project::GitHub(repo) => {
                    let read = work.gh.read(repo).await;
                    work.snapshot.send_modify(|snapshot| {
                        let ProjectView::Github(view) =
                            &mut Arc::make_mut(snapshot).projects[index]
                        else {
                            unreachable!("views follow the configuration")
                        };
                        match read {
                            Ok(fresh) => {
                                view.work = Some(fresh);
                                view.synced = Some(unix_ms());
                                view.error = None;
                            }
                            Err(error) => view.error = Some(format!("{error:#}")),
                        }
                    });
                }
                Project::Beads(store) => {
                    let marker = beads::marker(&store.folder);
                    let due = due(attempted.get(&index), &marker);
                    let unseen = marker.as_ref().err().cloned();
                    attempted.insert(index, marker);
                    if !due {
                        work.snapshot.send_if_modified(|snapshot| {
                            let ProjectView::Beads(view) =
                                &mut Arc::make_mut(snapshot).projects[index]
                            else {
                                unreachable!("views follow the configuration")
                            };
                            let changed = view.unwatched != unseen;
                            view.unwatched = unseen;
                            changed
                        });
                        continue;
                    }
                    let read = work.store(&store.folder).await;
                    work.snapshot.send_if_modified(|snapshot| {
                        let ProjectView::Beads(view) = &mut Arc::make_mut(snapshot).projects[index]
                        else {
                            unreachable!("views follow the configuration")
                        };
                        let unwatched_changed = view.unwatched != unseen;
                        view.unwatched = unseen;
                        match read {
                            Ok((_, false)) if view.error.is_none() && view.items.is_some() => {
                                unwatched_changed
                            }
                            Ok((items, _)) => {
                                view.items = Some(items.to_vec());
                                view.synced = Some(unix_ms());
                                view.error = None;
                                true
                            }
                            Err(error) => {
                                view.error = Some(format!("{error:#}"));
                                true
                            }
                        }
                    });
                }
            }
        }
        if github {
            work.poll.lock().unwrap().last_github = Some(Instant::now());
        }
        // A refresh is only ever asked for; GitHub is not read on a timer.
        tokio::select! {
            () = tokio::time::sleep(EVERY) => {}
            () = work.refresh.notified() => {}
        }
        github = false;
    }
}

/// How long until GitHub may be read again.
fn github_wait(work: &Inner) -> Duration {
    work.poll
        .lock()
        .unwrap()
        .last_github
        .map_or(Duration::ZERO, |at| EVERY.saturating_sub(at.elapsed()))
}

/// Whether a store is read this round, given its marker at this session's
/// last attempt and now: always at a session's first round, then only when
/// the marker changed. A store without one waits for the next session.
fn due(before: Option<&Result<Vec<u8>, String>>, now: &Result<Vec<u8>, String>) -> bool {
    match (before, now) {
        (None, _) => true,
        (Some(Ok(before)), Ok(now)) => before != now,
        (Some(Err(_)), Ok(_)) => true,
        (Some(_), Err(_)) => false,
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(all(test, unix))]
pub(super) mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::work::config::{Repo, Store};

    /// A scratch folder with stand-ins for gh and bd that log their
    /// arguments and answer with a recorded fixture, or fail as the real
    /// ones do, and a project folder holding a store whose manifest a test
    /// can change.
    pub(in crate::work) struct Fake {
        pub dir: PathBuf,
    }

    impl Fake {
        pub fn new(test: &str, gh: &str, bd: &str) -> Fake {
            let dir = std::env::temp_dir().join(format!("parvi-179-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("store/.beads/embeddeddolt/tst/.dolt/noms")).unwrap();
            let fake = Fake { dir };
            fake.touch("one");
            for (name, log, answer) in [("gh", "$4 $6", gh), ("bd", "$*", bd)] {
                crate::test_support::write_script(
                    &fake.dir.join(name),
                    &format!(
                        "#!/bin/sh\necho \"{log}\" >> '{}/{name}.runs'\n{answer}\n",
                        fake.dir.display()
                    ),
                );
            }
            fake
        }

        /// Writes the store's manifest, as a write through bd would.
        pub fn touch(&self, manifest: &str) {
            std::fs::write(
                self.dir
                    .join("store/.beads/embeddeddolt/tst/.dolt/noms/manifest"),
                manifest,
            )
            .unwrap();
        }

        pub fn store(&self) -> String {
            self.dir.join("store").display().to_string()
        }

        pub fn work(&self, projects: Vec<Project>) -> Work {
            Work::new(
                Config { projects },
                "n1",
                Gh::new(self.dir.join("gh").display().to_string()),
                Bd::new(self.dir.join("bd").display().to_string()),
            )
        }

        pub fn runs(&self, tool: &str) -> Vec<String> {
            std::fs::read_to_string(self.dir.join(format!("{tool}.runs")))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    pub(in crate::work) fn fixture(name: &str) -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/work/");
        format!("cat '{path}{name}'")
    }

    fn parvi() -> Project {
        Project::GitHub(Repo {
            name: "example-owner/parvi".into(),
            checkout: Some("~/parvi".into()),
        })
    }

    fn repo(snapshot: &Snapshot, index: usize) -> &RepoView {
        match &snapshot.projects[index] {
            ProjectView::Github(view) => view,
            ProjectView::Beads(_) => panic!("not a repository"),
        }
    }

    fn store(snapshot: &Snapshot, index: usize) -> &StoreView {
        match &snapshot.projects[index] {
            ProjectView::Beads(view) => view,
            ProjectView::Github(_) => panic!("not a store"),
        }
    }

    #[tokio::test]
    async fn reads_only_while_watched() {
        let fake = Fake::new("watched", &fixture("parvi.json"), &fixture("beads.json"));
        let work = fake.work(vec![
            parvi(),
            Project::Beads(Store {
                folder: fake.store(),
            }),
        ]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            fake.runs("gh").is_empty() && fake.runs("bd").is_empty(),
            "read with nobody watching"
        );

        let mut watching = work.subscribe();
        let first = watching.next().await;
        assert_eq!(repo(&first, 0).checkout.as_deref(), Some("~/parvi"));
        assert_eq!(store(&first, 1).items, None);
        let mut read = watching.next().await;
        while store(&read, 1).items.is_none() {
            read = watching.next().await;
        }
        assert_eq!(fake.runs("gh"), vec!["owner=example-owner name=parvi"]);
        assert_eq!(fake.runs("bd"), vec!["--json list --status all --limit 0"]);
        assert!(repo(&read, 0).synced.is_some());
        assert_eq!(repo(&read, 0).work.as_ref().unwrap().open_issues, 15);
        assert_eq!(store(&read, 1).items.as_ref().unwrap().len(), 4);

        drop(watching);
        assert!(work.0.poll.lock().unwrap().task.is_none());
        // Back within a round: the snapshot at once, GitHub not again until
        // due, and the store, unchanged, not read again.
        let mut again = work.subscribe();
        let snapshot = again.next().await;
        assert_eq!(repo(&snapshot, 0).work.as_ref().unwrap().open_issues, 15);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(fake.runs("gh").len(), 1);
        assert_eq!(fake.runs("bd").len(), 1);
    }

    #[tokio::test]
    async fn reads_a_store_again_only_after_it_changed() {
        let fake = Fake::new("changed", "exit 1", &fixture("beads.json"));
        let folder = fake.store();
        let work = fake.work(vec![Project::Beads(Store {
            folder: folder.clone(),
        })]);
        let (items, read) = work.0.store(&folder).await.unwrap();
        assert!(read && items.len() == 4);
        assert!(
            !work.0.store(&folder).await.unwrap().1,
            "unchanged: no read"
        );
        fake.touch("two");
        assert!(
            work.0.store(&folder).await.unwrap().1,
            "changed: read again"
        );
        assert_eq!(fake.runs("bd").len(), 2);
    }

    #[tokio::test]
    async fn a_failed_read_keeps_what_was_read_and_says_why() {
        let fake = Fake::new(
            "failed",
            "echo 'gh: HTTP 504' >&2; exit 1",
            "echo 'Error: database locked' >&2; exit 1",
        );
        let work = fake.work(vec![
            parvi(),
            Project::Beads(Store {
                folder: fake.store(),
            }),
        ]);
        let mut watching = work.subscribe();
        watching.next().await;
        let mut read = watching.next().await;
        while store(&read, 1).error.is_none() {
            read = watching.next().await;
        }
        assert_eq!(repo(&read, 0).error.as_deref(), Some("HTTP 504"));
        assert_eq!(repo(&read, 0).synced, None);
        // A store that failed shows as failed, never as an empty store.
        assert_eq!(store(&read, 1).error.as_deref(), Some("database locked"));
        assert_eq!(store(&read, 1).items, None);
    }

    #[test]
    fn a_store_is_read_again_only_when_its_marker_changes() {
        let one: Result<Vec<u8>, String> = Ok(b"one".to_vec());
        let two: Result<Vec<u8>, String> = Ok(b"two".to_vec());
        let none: Result<Vec<u8>, String> = Err("no manifest".into());
        assert!(
            due(None, &one) && due(None, &none),
            "a session's first round"
        );
        assert!(!due(Some(&one), &one), "unchanged");
        assert!(due(Some(&one), &two), "written");
        assert!(due(Some(&none), &one), "a manifest appeared");
        assert!(!due(Some(&none), &none), "no marker: never by itself");
        assert!(!due(Some(&one), &none));
    }

    #[tokio::test]
    async fn a_store_without_a_manifest_is_read_once_per_watch_and_says_so() {
        let fake = Fake::new("unwatched", "exit 1", &fixture("beads.json"));
        std::fs::remove_dir_all(fake.dir.join("store/.beads/embeddeddolt")).unwrap();
        let work = fake.work(vec![Project::Beads(Store {
            folder: fake.store(),
        })]);
        let mut watching = work.subscribe();
        let mut read = watching.next().await;
        while store(&read, 0).items.is_none() {
            read = watching.next().await;
        }
        let unwatched = store(&read, 0).unwatched.as_deref().unwrap();
        assert!(unwatched.contains("no Dolt manifest"), "{unwatched}");
        assert_eq!(fake.runs("bd").len(), 1);
        // A store that keeps its manifest beside a Dolt server is watched.
        std::fs::create_dir_all(fake.dir.join("store/.beads/dolt/tst/.dolt/noms")).unwrap();
        std::fs::write(
            fake.dir.join("store/.beads/dolt/tst/.dolt/noms/manifest"),
            "m",
        )
        .unwrap();
        assert!(beads::marker(&fake.store()).is_ok());
    }

    /// Watching anew reads a store at once, even right after GitHub was
    /// read, which waits for its round.
    #[tokio::test]
    async fn a_new_watch_reads_the_stores_at_once() {
        let fake = Fake::new("rewatch", &fixture("parvi.json"), &fixture("beads.json"));
        std::fs::remove_dir_all(fake.dir.join("store/.beads/embeddeddolt")).unwrap();
        let work = fake.work(vec![
            parvi(),
            Project::Beads(Store {
                folder: fake.store(),
            }),
        ]);
        let mut watching = work.subscribe();
        let mut read = watching.next().await;
        while store(&read, 1).items.is_none() || repo(&read, 0).synced.is_none() {
            read = watching.next().await;
        }
        drop(watching);

        let mut again = work.subscribe();
        again.next().await;
        // Waiting for GitHub's round would take the rest of its minute.
        let read_again = async {
            while fake.runs("bd").len() < 2 {
                again.next().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), read_again)
            .await
            .expect("the store was not read when watching started");
        assert_eq!(fake.runs("bd").len(), 2);
        assert_eq!(fake.runs("gh").len(), 1, "GitHub read again before due");
    }

    #[tokio::test]
    async fn github_is_read_again_on_a_refresh_and_never_on_a_timer() {
        let fake = Fake::new("refresh", &fixture("parvi.json"), &fixture("beads.json"));
        let work = fake.work(vec![parvi()]);
        let mut watching = work.subscribe();
        let mut read = watching.next().await;
        while repo(&read, 0).synced.is_none() {
            read = watching.next().await;
        }
        assert_eq!(fake.runs("gh").len(), 1);

        // Rounds pass while the tab stays open: GitHub is not read again.
        tokio::time::pause();
        for _ in 0..3 {
            tokio::time::advance(EVERY).await;
            tokio::task::yield_now().await;
        }
        tokio::time::resume();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(fake.runs("gh").len(), 1, "GitHub read on a timer");

        work.refresh();
        let read_again = async {
            while fake.runs("gh").len() < 2 {
                watching.next().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), read_again)
            .await
            .expect("a refresh did not read GitHub");
    }

    #[tokio::test]
    async fn no_projects_means_no_poller() {
        let work = Work::default();
        let mut watching = work.subscribe();
        assert!(watching.next().await.projects.is_empty());
        assert!(work.0.poll.lock().unwrap().task.is_none());
    }
}
