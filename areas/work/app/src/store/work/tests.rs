//! The Work page's rules, against CHROTE's own cases (`beadsSlice.test.ts`),
//! for Beads stores and GitHub repositories alike.

use super::outline::{self, Line};
use super::*;
use parvi_work_contract::{Item, RepoView as Repo, StoreView as Store};

/// Noon UTC on 2026-10-21, the day CHROTE's cases are written for.
const NOW: u64 = 1_792_584_000_000;

fn bead(id: &str, status: &str, updated: &str) -> Item {
    Item {
        reference: format!("bead:n:/srv/x#{id}"),
        id: id.into(),
        title: format!("Title of {id}"),
        status: status.into(),
        kind: Some("task".into()),
        created: "2026-11-01T00:00:00Z".into(),
        updated: updated.into(),
        ..Item::default()
    }
}

fn board() -> Board {
    let mut blocked = bead("x-3", "open", "2026-11-03T00:00:00Z");
    blocked.family.blockers = vec!["bead:n:/srv/x#x-1".into()];
    let mut done_blocker = bead("x-4", "open", "2026-11-04T00:00:00Z");
    done_blocker.family.blockers = vec!["x-2".into()];
    let s = Snapshot {
        refreshed: 0,
        projects: vec![
            Project::Beads(Store {
                folder: "/srv/x".into(),
                unwatched: Some("no watch".into()),
                items: Some(vec![
                    bead("x-1", "in_progress", "2026-11-05T00:00:00Z"),
                    bead("x-2", "closed", "2026-11-02T00:00:00Z"),
                    blocked,
                    done_blocker,
                ]),
                ..Store::default()
            }),
            Project::Github(Repo {
                name: "o/r".into(),
                work: Some(serde_json::from_value(serde_json::json!({
                    "url":"", "openIssues":1, "closedIssues":0, "openPulls":0, "closedPulls":0,
                    "issues":[{"number":10,"title":"Crash on start","url":"", "state":"open", "created":"", "updated":"2026-11-06T00:00:00Z", "body":"", "labels":[], "assignees":[], "branches":[], "pulls":[]}], "pulls":[]
                })).unwrap()),
                ..Repo::default()
            }),
            Project::Github(Repo {
                name: "o/broken".into(),
                error: Some("gh: not logged in".into()),
                ..Repo::default()
            }),
        ],
    };
    Board::from_snapshot(&s)
}

#[test]
fn rows_come_from_both_trackers_and_failures_are_not_empty_work() {
    let b = board();
    assert_eq!(b.rows.len(), 5);
    assert_eq!(b.projects[0].label, "x");
    assert!(b.projects[0].read);
    assert!(!b.projects[2].read);
    assert_eq!(b.projects[2].error.as_deref(), Some("gh: not logged in"));
    let issue = &b.rows[b.find("gh:o/r#10").unwrap()];
    assert_eq!(issue.id, "#10");
    assert_eq!(issue.line(&b), "issue o/r#10: Crash on start");
    assert_eq!(b.rows[0].line(&b), "bead x-1: Title of x-1");
}

#[test]
fn an_open_bead_with_an_unfinished_blocker_is_blocked() {
    let b = board();
    let status = |id: &str| b.rows.iter().find(|r| r.id == id).unwrap().status;
    assert_eq!(status("x-3"), Status::Blocked);
    // Its only blocker is closed (and named by its bare id).
    assert_eq!(status("x-4"), Status::Open);
}

// CHROTE's cases, as the node would send them: a Bead of the same store
// named canonically, one of another store by its bare id.

fn at(day: &str) -> String {
    format!("{day}T12:00:00Z")
}

/// A Bead as CHROTE's cases write one: open, created and updated on Oct 1.
pub fn b(id: &str) -> Item {
    Item {
        id: id.into(),
        title: format!("Title of {id}"),
        status: "open".into(),
        kind: Some("task".into()),
        priority: Some(2),
        created: at("2026-10-01"),
        updated: at("2026-10-01"),
        ..Item::default()
    }
}

pub trait Bead: Sized {
    fn with(self, f: impl FnOnce(&mut Item)) -> Self;
    fn status(self, s: &str) -> Self {
        self.with(|i| i.status = s.into())
    }
    fn parent(self, p: &str) -> Self {
        self.with(|i| i.family.parent = Some(p.into()))
    }
    fn blocked_by(self, ids: &[&str]) -> Self {
        self.with(|i| i.family.blockers = ids.iter().map(|s| s.to_string()).collect())
    }
    fn created(self, day: &str) -> Self {
        self.with(|i| i.created = at(day))
    }
    fn updated(self, day: &str) -> Self {
        self.with(|i| i.updated = at(day))
    }
    fn closed(self, day: &str) -> Self {
        self.with(|i| i.closed = Some(at(day)))
    }
    fn title(self, t: &str) -> Self {
        self.with(|i| i.title = t.into())
    }
    fn body(self, t: &str) -> Self {
        self.with(|i| i.body = t.into())
    }
    fn epic(self) -> Self {
        self.with(|i| i.kind = Some("epic".into()))
    }
}

impl Bead for Item {
    fn with(mut self, f: impl FnOnce(&mut Item)) -> Self {
        f(&mut self);
        self
    }
}

pub fn store(folder: &str, mut items: Vec<Item>) -> Project {
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let name = |id: &str| {
        if ids.iter().any(|x| x == id) {
            format!("bead:n:{folder}#{id}")
        } else {
            id.to_string()
        }
    };
    for i in &mut items {
        i.reference = name(&i.id);
        i.family.parent = i.family.parent.as_deref().map(name);
        i.family.blockers = i.family.blockers.iter().map(|x| name(x)).collect();
    }
    Project::Beads(Store {
        folder: folder.into(),
        synced: Some(NOW),
        items: Some(items),
        ..Store::default()
    })
}

fn board_of(projects: Vec<Project>) -> Board {
    Board::from_snapshot(&Snapshot {
        refreshed: 0,
        projects,
    })
}

#[test]
fn an_on_demand_card_rechecks_a_completed_blocker_when_the_list_arrives() {
    let project = store(
        "/x",
        vec![b("x-1").blocked_by(&["x-2"]), b("x-2").status("closed")],
    );
    let Project::Beads(store) = &project else {
        unreachable!()
    };
    let item = store
        .items
        .as_ref()
        .unwrap()
        .iter()
        .find(|i| i.id == "x-1")
        .unwrap()
        .clone();
    let board = Board::from_item(item, &[project]);
    assert_eq!(board.rows[0].status, Status::Open);
    assert_eq!(board.links.blockers[0].len(), 1);
}

fn row(b: &Board, id: &str) -> usize {
    b.rows.iter().position(|r| r.id == id).unwrap()
}

fn status(b: &Board, id: &str) -> Status {
    b.rows[row(b, id)].status
}

/// A repository with issue #10, closed by pull #11, and issue #12.
pub fn repo() -> Project {
    Project::Github(Repo {
        name: "o/r".into(),
        synced: Some(NOW),
        work: Some(serde_json::from_value(serde_json::json!({
            "url":"", "openIssues":1, "closedIssues":1, "openPulls":1, "closedPulls":0,
            "issues":[
                {"number":10,"title":"Crash on start","url":"", "state":"open", "created":at("2026-10-02"), "updated":at("2026-10-20"), "body":"It dies before the first frame.", "labels":[], "assignees":[], "branches":[], "pulls":[11]},
                {"number":12,"title":"Docs typo","url":"", "state":"closed", "created":at("2026-10-03"), "updated":at("2026-10-04"), "body":"", "labels":[], "assignees":[], "branches":[], "pulls":[]}
            ],
            "pulls":[{"number":11,"title":"Start without a frame", "url":"", "state":"open", "created":at("2026-10-19"), "updated":at("2026-10-19"), "body":"Closes #10, with a much longer description than the issue has.", "head":"", "base":"", "additions":0, "deletions":0, "labels":[], "assignees":[], "closes":[10]}]
        })).unwrap()),
        ..Repo::default()
    })
}

/// beadsSlice.test.ts, links across stores.
fn linked() -> Board {
    board_of(vec![
        store(
            "/srv/data/context",
            vec![
                b("library-zvi").epic(),
                b("library-zvi.1")
                    .parent("library-zvi")
                    .blocked_by(&["ctx-xvl", "srv-lbpn"]),
                b("library-zvi.2")
                    .parent("library-zvi")
                    .blocked_by(&["library-zvi.1"]),
            ],
        ),
        store(
            "/srv/example-store",
            vec![
                b("ctx-xvl").status("closed").closed("2026-10-20"),
                b("ctx-child").parent("library-zvi"),
            ],
        ),
        store("/srv", vec![b("srv-lbpn")]),
    ])
}

#[test]
fn counts_a_child_in_another_store_among_its_parents_kids() {
    let b = linked();
    assert_eq!(b.links.children[row(&b, "library-zvi")].len(), 3);
    assert_eq!(
        b.links.parent[row(&b, "ctx-child")],
        Some(row(&b, "library-zvi"))
    );
}

#[test]
fn reads_a_bead_as_blocked_while_any_blocker_in_any_store_is_unfinished() {
    let lk = linked();
    assert_eq!(status(&lk, "library-zvi.1"), Status::Blocked);
    assert_eq!(status(&lk, "library-zvi.2"), Status::Blocked);
    let done = board_of(vec![
        store("/a", vec![b("a-1").blocked_by(&["b-1"])]),
        store("/b", vec![b("b-1").status("closed")]),
    ]);
    assert_eq!(status(&done, "a-1"), Status::Open);
}

#[test]
fn counts_a_blocker_no_store_holds_as_unfinished() {
    let lost = board_of(vec![store("/a", vec![b("a-1").blocked_by(&["gone-1"])])]);
    assert_eq!(status(&lost, "a-1"), Status::Blocked);
    assert_eq!(lost.links.unresolved[0], vec!["gone-1".to_string()]);
}

#[test]
fn resolves_an_id_in_its_own_store_first_and_refuses_a_guess_between_two_others() {
    let shared = board_of(vec![
        store(
            "/tmp/fixture/alice",
            vec![b("home-1"), b("home-2").parent("home-1")],
        ),
        store(
            "/tmp/fixture/bob",
            vec![b("home-1"), b("home-3").parent("home-1")],
        ),
    ]);
    assert_eq!(shared.links.parent[3], Some(2));
    assert_eq!(shared.links.parent[1], Some(0));
    assert_eq!(shared.find("home-1"), None);
    assert_eq!(shared.find_from("home-1", 1), Some(2));
    // A third store's child of home-1 could mean either: it has no parent.
    let third = board_of(vec![
        store("/tmp/fixture/alice", vec![b("home-1")]),
        store("/tmp/fixture/bob", vec![b("home-1")]),
        store("/c", vec![b("c-1").parent("home-1")]),
    ]);
    assert_eq!(third.links.parent[2], None);
}

#[test]
fn waits_on_every_stores_bead_of_a_blocker_id_two_other_stores_hold_until_all_are_finished() {
    let moved = |c: &str| {
        board_of(vec![
            store("/a", vec![b("a-1").blocked_by(&["x-1"])]),
            store("/b", vec![b("x-1").status("closed")]),
            store("/c", vec![b("x-1").status(c)]),
        ])
    };
    assert_eq!(status(&moved("closed"), "a-1"), Status::Open);
    let waiting = moved("open");
    assert_eq!(status(&waiting, "a-1"), Status::Blocked);
    assert_eq!(waiting.links.blockers[0], vec![1, 2]);
    assert!(waiting.links.unresolved[0].is_empty());
}

#[test]
fn names_an_item_to_an_agent_with_its_stores_folder_when_the_id_alone_could_mean_two_stores() {
    let shared = board_of(vec![
        store("/tmp/fixture/alice", vec![b("home-1").title("Mine")]),
        store("/tmp/fixture/bob", vec![b("home-2").title("Theirs")]),
        store(
            "/srv",
            vec![
                b("srv-1").title("Ours"),
                b("srv-2"),
                b("ctx-9").title("Moved"),
            ],
        ),
        store("/srv/example-store", vec![b("ctx-9").title("Moved")]),
        repo(),
    ]);
    let line = |i: usize| shared.rows[i].line(&shared);
    assert_eq!(line(0), "bead home-1 (/tmp/fixture/alice): Mine");
    assert_eq!(line(2), "bead srv-1: Ours");
    assert_eq!(line(4), "bead ctx-9 (/srv): Moved");
    let issue = shared.find("gh:o/r#10").unwrap();
    assert_eq!(line(issue), "issue o/r#10: Crash on start");
    let pull = shared.pull(4, 11).unwrap();
    assert_eq!(line(pull), "pull o/r#11: Start without a frame");
}

#[test]
fn names_a_shared_prefix_by_its_folder_so_two_homes_never_read_alike() {
    let b = board_of(vec![
        store("/tmp/fixture/alice", vec![b("home-1")]),
        store("/tmp/fixture/bob", vec![b("home-1")]),
        // A store holding a Bead moved in from another keeps its own name.
        store("/srv", vec![b("ctx-9"), b("srv-1"), b("srv-2")]),
        store("/srv/unnamed", vec![]),
        repo(),
    ]);
    let labels: Vec<&str> = b.projects.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(
        labels,
        ["home (alice)", "home (bob)", "srv", "unnamed", "o/r"]
    );
}

fn lines(b: &Board, project: usize) -> Vec<(String, usize)> {
    outline::tree(b, project)
        .into_iter()
        .map(|Line { row, depth }| (b.rows[row].id.clone(), depth))
        .collect()
}

#[test]
fn a_project_opens_as_its_open_work_under_its_epics_most_pressing_first() {
    let b = board_of(vec![
        store(
            "/srv/p",
            vec![
                b("p-1").title("Old idea").updated("2026-10-02"),
                b("p-2").epic().updated("2026-10-05"),
                b("p-2.10").parent("p-2"),
                b("p-2.2").parent("p-2").blocked_by(&["p-2.10"]),
                b("p-2.1").parent("p-2").status("closed"),
                b("p-2.3").parent("p-2"),
                b("p-2.3.1").parent("p-2.3").status("in_progress"),
                b("p-3").status("deferred"),
                b("p-4").status("closed"),
                b("p-5").updated("2026-10-09"),
                b("p-6").parent("p-4"),
            ],
        ),
        store("/srv/q", vec![b("q-1").parent("p-2")]),
    ]);
    // The epic holds work under way, so it leads; its children under it,
    // under way first, then ready by id, then blocked. A child of finished
    // work stands as a root; deferred and finished work is left out.
    assert_eq!(
        lines(&b, 0),
        [
            ("p-2", 0),
            ("p-2.3", 1),
            ("p-2.3.1", 2),
            ("p-2.10", 1),
            ("q-1", 1),
            ("p-2.2", 1),
            ("p-5", 0),
            ("p-1", 0),
            ("p-6", 0),
        ]
        .map(|(id, d)| (id.to_string(), d))
    );
    // A child filed from another store is its project's own root too.
    assert_eq!(lines(&b, 1), [("q-1".to_string(), 0)]);
    assert_eq!(outline::open_counts(&b), [8, 1]);
}

#[test]
fn a_search_finds_this_projects_matches_first_open_before_finished() {
    let b = board_of(vec![
        store(
            "/srv/p",
            vec![
                b("p-1")
                    .title("Read the stores")
                    .status("closed")
                    .updated("2026-10-09"),
                b("p-2")
                    .title("Show what the stores hold")
                    .updated("2026-10-02"),
                b("p-3").title("Unrelated"),
            ],
        ),
        store("/srv/q", vec![b("q-1").body("Which stores to read")]),
    ]);
    let found = outline::search(&b, Some(0), "STORES");
    let ids =
        |rows: &[usize]| -> Vec<&str> { rows.iter().map(|&i| b.rows[i].id.as_str()).collect() };
    assert_eq!(ids(&found.here), ["p-2", "p-1"]);
    assert_eq!(ids(&found.elsewhere), ["q-1"]);
    assert_eq!(found.more, 0);
    assert_eq!(
        outline::search(&b, Some(0), "  "),
        outline::Found::default()
    );
    assert_eq!(
        ids(&outline::search(&b, None, "read stores").elsewhere),
        ["q-1", "p-1"]
    );
}

#[test]
fn a_sessions_folder_names_the_project_that_holds_it_most_closely() {
    let b = board_of(vec![
        store("/srv", vec![b("srv-1")]),
        store("/srv/games/korpi", vec![b("korpi-1")]),
        repo(),
    ]);
    assert_eq!(
        outline::project_of_folder(&b, "/srv/games/korpi/src"),
        Some(1)
    );
    assert_eq!(outline::project_of_folder(&b, "/srv/games/korpi/"), Some(1));
    assert_eq!(outline::project_of_folder(&b, "/srv/games/korpix"), Some(0));
    assert_eq!(outline::project_of_folder(&b, "/home/me"), None);
}
