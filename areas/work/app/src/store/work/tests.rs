//! The Work tab's rules, against CHROTE's own cases (`beadsSlice.test.ts`,
//! `workGraph.test.ts`), for Beads stores and GitHub repositories alike.

use super::days::{RECENT_DAYS, day_label, local_day, strip_start};
use super::graph::{self, NODE_HEIGHT, NODE_WIDTH, Orientation, READABLE_SCALE, WHOLE_SCALE};
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
    Board::from_snapshot(&s, NOW, &|_| 0)
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

#[test]
fn the_slice_filters_counts_and_sorts() {
    let b = board();
    let mut s = Slice::default();
    let ids = |s: &Slice| -> Vec<String> {
        s.apply(&b)
            .rows
            .iter()
            .map(|&i| b.rows[i].id.clone())
            .collect()
    };
    assert_eq!(ids(&s), ["#10", "x-1", "x-4", "x-3", "x-2"]);
    s.toggle_status(Status::Closed);
    assert_eq!(ids(&s), ["#10", "x-1", "x-4", "x-3"]);
    let r = s.apply(&b);
    // A status's count is what choosing it would give.
    assert_eq!(r.status_counts[Status::Closed as usize], 1);
    assert_eq!(r.project_counts, vec![3, 1, 0]);
    s.only_project("/srv/x");
    s.sort_by(Column::Id);
    assert_eq!(ids(&s), ["x-1", "x-3", "x-4"]);
    s.query = "#10".into();
    s.projects = None;
    assert_eq!(ids(&s), ["#10"]);
    s.query = "TITLE x-3".into();
    assert_eq!(ids(&s), ["x-3"]);
    // Toggling every status back in reads as no filter.
    s.toggle_status(Status::Closed);
    assert_eq!(s.statuses, None);
    s.only_project("/srv/x");
    s.only_project("/srv/x");
    assert_eq!(s.projects, None);
}

// CHROTE's cases, as the node would send them: a Bead of the same store
// named canonically, one of another store by its bare id.

fn at(day: &str) -> String {
    format!("{day}T12:00:00Z")
}

fn ms(day: &str) -> u64 {
    when::parse(&at(day)).unwrap()
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
    Board::from_snapshot(&Snapshot { projects }, NOW, &|_| 0)
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
    let board = Board::from_item(item, &[project], NOW);
    assert_eq!(board.rows[0].status, Status::Open);
    assert_eq!(board.links.blockers[0].len(), 1);
}

fn row(b: &Board, id: &str) -> usize {
    b.rows.iter().position(|r| r.id == id).unwrap()
}

fn status(b: &Board, id: &str) -> Status {
    b.rows[row(b, id)].status
}

fn shown(b: &Board, s: &Slice) -> Vec<String> {
    s.apply(b)
        .rows
        .iter()
        .map(|&i| b.rows[i].id.clone())
        .collect()
}

fn shown_sorted(b: &Board, s: &Slice) -> Vec<String> {
    let mut v = shown(b, s);
    v.sort();
    v
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
    let mut s = Slice::default();
    s.sort_by(Column::Kids);
    assert_eq!(shown(&b, &s)[0], "library-zvi");
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

/// beadsSlice.test.ts, the strip: an old store with one Bead open since the
/// start, and a store arriving on Oct 10 already holding three Beads, one
/// closed the next day.
fn strip_board() -> Board {
    board_of(vec![
        store("/old", vec![b("old-1").created("2026-10-01")]),
        store(
            "/new",
            vec![
                b("new-1").created("2026-10-10"),
                b("new-2").created("2026-10-10"),
                b("new-3")
                    .created("2026-10-10")
                    .status("closed")
                    .closed("2026-10-11"),
            ],
        ),
    ])
}

fn day(d: &str) -> i32 {
    local_day(ms(d), 0)
}

#[test]
fn counts_an_item_open_only_from_its_own_creation() {
    let b = strip_board();
    let r = Slice::default().apply(&b);
    let first = day("2026-10-01");
    let at = |d: &str| (day(d) - first) as usize;
    assert_eq!(b.days.first, first);
    assert_eq!(r.open[at("2026-10-09")], 1);
    assert_eq!(r.open[at("2026-10-10")], 4);
    assert_eq!(r.open[at("2026-10-11")], 3);
    assert_eq!(r.open[at("2026-10-21")], 3);
    assert_eq!(r.created[at("2026-10-10")], 3);
    assert_eq!(r.closed[at("2026-10-11")], 1);
}

#[test]
fn spans_the_recent_days_unless_every_day_is_asked_for_or_a_range_starts_earlier() {
    assert_eq!(strip_start(200, false, None, 1000), 200 - RECENT_DAYS);
    assert_eq!(strip_start(200, true, None, 1000), 0);
    assert_eq!(strip_start(RECENT_DAYS - 5, false, None, 1000), 0);
    assert_eq!(strip_start(200, false, Some(1030), 1000), 30);
    assert_eq!(strip_start(200, false, Some(1190), 1000), 200 - RECENT_DAYS);
}

#[test]
fn marks_the_day_a_project_was_first_seen() {
    let b = strip_board();
    let first = day("2026-10-01");
    let firsts: Vec<i32> = b
        .days
        .project_first
        .iter()
        .map(|d| d.unwrap() - first)
        .collect();
    assert_eq!(firsts, [0, day("2026-10-10") - first]);
}

#[test]
fn closes_an_item_stamped_closed_before_it_was_created_on_its_creation_day() {
    let odd = board_of(vec![store(
        "/a",
        vec![
            b("a-0").created("2026-10-01"),
            b("a-1")
                .created("2026-10-05")
                .status("closed")
                .closed("2026-10-03"),
        ],
    )]);
    let r = Slice::default().apply(&odd);
    let d = (day("2026-10-05") - odd.days.first) as usize;
    assert_eq!(r.closed[d], 1);
    assert_eq!(r.open[d], 1);
}

#[test]
fn leaves_a_date_written_as_zero_or_before_2000_off_the_strip() {
    let odd = board_of(vec![store(
        "/a",
        vec![
            b("a-1").created("2026-10-01"),
            b("a-2").with(|i| {
                i.created = "1970-01-01T00:00:00Z".into();
                i.updated = "0001-01-01T00:00:00Z".into();
                i.closed = Some("1970-01-01T00:00:01Z".into());
            }),
        ],
    )]);
    let d = &odd.days;
    assert_eq!(d.first, day("2026-10-01"));
    assert_eq!(
        (d.created[1], d.updated[1], d.closed[1]),
        (None, None, None)
    );
}

#[test]
fn closes_a_finished_item_with_no_close_time_on_its_last_update() {
    let odd = board_of(vec![store(
        "/a",
        vec![
            b("a-1")
                .created("2026-10-01")
                .updated("2026-10-04")
                .status("closed"),
        ],
    )]);
    let r = Slice::default().apply(&odd);
    assert_eq!(r.open[(day("2026-10-05") - odd.days.first) as usize], 0);
    // A closed GitHub issue carries no close time: it closes on its update.
    let gh = board_of(vec![repo()]);
    let typo = gh.find("gh:o/r#12").unwrap();
    assert_eq!(gh.days.closed[typo], Some(day("2026-10-04")));
}

/// beadsSlice.test.ts, filtering.
fn filtering_board() -> Board {
    board_of(vec![
        store(
            "/a",
            vec![
                b("a-1")
                    .title("Grill brain ownership")
                    .created("2026-10-02")
                    .updated("2026-10-02"),
                b("a-2")
                    .title("Something else")
                    .status("closed")
                    .created("2026-10-02")
                    .updated("2026-10-20")
                    .closed("2026-10-20"),
                b("a-3")
                    .title("Move records")
                    .status("in_progress")
                    .created("2026-10-05")
                    .updated("2026-10-06")
                    .body("Cite the evidence."),
            ],
        ),
        store(
            "/b",
            vec![
                b("b-1")
                    .title("Brain catalogue")
                    .created("2026-10-03")
                    .updated("2026-10-03"),
            ],
        ),
    ])
}

fn query(b: &Board, q: &str) -> Vec<String> {
    shown_sorted(
        b,
        &Slice {
            query: q.into(),
            ..Slice::default()
        },
    )
}

#[test]
fn keeps_an_item_only_when_every_term_is_in_its_id_title_or_text() {
    let b = filtering_board();
    assert_eq!(query(&b, "BRAIN"), ["a-1", "b-1"]);
    assert_eq!(query(&b, "brain grill"), ["a-1"]);
    // CHROTE asks its server for the text; the snapshot carries it.
    assert_eq!(query(&b, "evidence"), ["a-3"]);
    let gh = board_of(vec![repo()]);
    // The pull request that closes #10 says so in its text.
    assert_eq!(query(&gh, "#10"), ["#10", "#11"]);
    assert_eq!(query(&gh, "first frame"), ["#10"]);
}

#[test]
fn counts_each_facet_without_its_own_filter() {
    let b = filtering_board();
    let mut s = Slice::default();
    s.only_project("/b");
    s.only_status(Status::Open);
    let r = s.apply(&b);
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.project_counts, [1, 1]);
    assert_eq!(r.status_counts[Status::Open as usize], 1);
    assert_eq!(r.status_counts[Status::Closed as usize], 0);
}

#[test]
fn keeps_an_item_created_updated_or_closed_in_the_range_and_leaves_the_strip_whole() {
    let b = filtering_board();
    let mut s = Slice {
        range: Some((day("2026-10-20"), day("2026-10-21"))),
        ..Slice::default()
    };
    assert_eq!(shown_sorted(&b, &s), ["a-2"]);
    assert_eq!(s.apply(&b).created.iter().sum::<u32>(), 4);
    s.range = Some((day("2026-10-06"), day("2026-10-06")));
    assert_eq!(shown_sorted(&b, &s), ["a-3"]);
    // GitHub's items take the same range.
    let gh = board_of(vec![repo()]);
    s.range = Some((day("2026-10-19"), day("2026-10-19")));
    assert_eq!(shown_sorted(&gh, &s), ["#11"]);
}

fn sorting_board() -> Board {
    let sized = |id: &str, n: usize, updated: &str| b(id).body(&"x".repeat(n)).updated(updated);
    board_of(vec![store(
        "/a",
        vec![
            sized("a-2", 900, "2026-10-01"),
            sized("a-10", 6400, "2026-10-20"),
            sized("a-1", 900, "2026-10-11"),
        ],
    )])
}

fn sorted(b: &Board, c: Column, descending: bool) -> Vec<String> {
    shown(
        b,
        &Slice {
            sort: c,
            descending,
            ..Slice::default()
        },
    )
}

#[test]
fn orders_ids_naturally() {
    assert_eq!(
        sorted(&sorting_board(), Column::Id, false),
        ["a-1", "a-2", "a-10"]
    );
    let gh = board_of(vec![repo()]);
    assert_eq!(sorted(&gh, Column::Id, false), ["#10", "#11", "#12"]);
}

#[test]
fn orders_by_size_largest_first_ties_by_id() {
    let b = sorting_board();
    assert_eq!(sorted(&b, Column::Size, true), ["a-10", "a-2", "a-1"]);
    // A first click sorts largest first.
    let mut s = Slice::default();
    s.sort_by(Column::Size);
    assert!(s.descending);
    let gh = board_of(vec![repo()]);
    assert_eq!(sorted(&gh, Column::Size, true), ["#11", "#10", "#12"]);
}

#[test]
fn keeps_only_the_visible_rows_in_order() {
    let b = sorting_board();
    let s = Slice {
        query: "a-1".into(),
        sort: Column::Size,
        ..Slice::default()
    };
    assert_eq!(shown(&b, &s), ["a-10", "a-1"]);
}

#[test]
fn says_a_day_as_the_strip_and_the_range_read_it() {
    assert_eq!(day_label(day("2026-10-20")), "Oct 20");
    assert_eq!(day_label(day("2027-01-02")), "Jan 2");
}

#[test]
fn takes_the_local_calendar_day() {
    assert_eq!(day("2026-10-21") - day("2026-10-20"), 1);
    // 23:00 UTC is the next day two hours east, the same day five west.
    let late = when::parse("2026-10-20T23:00:00Z").unwrap();
    assert_eq!(local_day(late, 2 * 3600), day("2026-10-21"));
    assert_eq!(local_day(late, -5 * 3600), day("2026-10-20"));
}

// workGraph.test.ts.

/// A Bead titled by its id, as workGraph.test.ts writes one.
fn g(id: &str) -> Item {
    b(id).title(id)
}

fn graph_board() -> Board {
    board_of(vec![
        store(
            "/srv/data/context",
            vec![
                g("library-zvi").epic(),
                g("library-zvi.1")
                    .parent("library-zvi")
                    .blocked_by(&["ctx-v8m", "srv-lbpn"]),
                g("library-zvi.2")
                    .parent("library-zvi")
                    .blocked_by(&["library-zvi.1"]),
                g("library-lone"),
            ],
        ),
        store(
            "/srv/example-store",
            vec![
                g("ctx-5t7").status("closed"),
                g("ctx-v8m").status("closed").blocked_by(&["ctx-5t7"]),
            ],
        ),
        store(
            "/srv",
            vec![g("srv-lbpn"), g("srv-later").blocked_by(&["library-zvi.2"])],
        ),
    ])
}

const WIDE: (f32, f32) = (1600., 600.);

fn ids_of(b: &Board, gr: &graph::Graph) -> Vec<String> {
    let mut v: Vec<String> = gr.nodes.iter().map(|n| b.rows[n.row].id.clone()).collect();
    v.sort();
    v
}

fn node<'a>(b: &Board, gr: &'a graph::Graph, id: &str) -> &'a graph::Node {
    gr.nodes.iter().find(|n| b.rows[n.row].id == id).unwrap()
}

#[test]
fn the_family_is_the_items_own_children_its_parents_otherwise_and_itself_alone_last() {
    let b = graph_board();
    let r = |id| row(&b, id);
    assert_eq!(graph::family_head(&b, r("library-zvi")), r("library-zvi"));
    assert_eq!(graph::family_head(&b, r("library-zvi.2")), r("library-zvi"));
    assert_eq!(graph::family_head(&b, r("library-lone")), r("library-lone"));
}

#[test]
fn keeps_the_same_drawing_as_the_selection_moves_between_siblings() {
    let b = graph_board();
    let one = graph::build(&b, row(&b, "library-zvi.1"), WIDE);
    let two = graph::build(&b, row(&b, "library-zvi.2"), WIDE);
    assert_eq!(one.nodes, two.nodes);
    assert_eq!(one.edges, two.edges);
}

#[test]
fn brings_in_what_blocks_the_family_or_waits_on_it_from_other_stores_with_its_edges() {
    let b = graph_board();
    let gr = graph::build(&b, row(&b, "library-zvi"), WIDE);
    assert_eq!(gr.orientation, Orientation::Across);
    assert_eq!(
        ids_of(&b, &gr),
        [
            "ctx-v8m",
            "library-zvi.1",
            "library-zvi.2",
            "srv-later",
            "srv-lbpn"
        ]
    );
    assert!(node(&b, &gr, "ctx-v8m").outside);
    assert!(!node(&b, &gr, "library-zvi.1").outside);
    let mut edges: Vec<String> = gr
        .edges
        .iter()
        .map(|e| {
            format!(
                "{}>{}",
                b.rows[gr.nodes[e.from].row].id, b.rows[gr.nodes[e.to].row].id
            )
        })
        .collect();
    edges.sort();
    assert_eq!(
        edges,
        [
            "ctx-v8m>library-zvi.1",
            "library-zvi.1>library-zvi.2",
            "library-zvi.2>srv-later",
            "srv-lbpn>library-zvi.1"
        ]
    );
    let x = |id| node(&b, &gr, id).x;
    assert!(x("ctx-v8m") < x("library-zvi.1"));
    assert!(x("library-zvi.1") < x("library-zvi.2"));
    assert!(x("library-zvi.2") < x("srv-later"));
}

#[test]
fn draws_an_issue_with_the_pull_requests_that_close_it() {
    let b = board_of(vec![repo()]);
    let issue = b.find("gh:o/r#10").unwrap();
    // A pull request does not make its issue read as blocked.
    assert_eq!(b.rows[issue].status, Status::Open);
    let gr = graph::build(&b, issue, WIDE);
    assert_eq!(ids_of(&b, &gr), ["#10", "#11"]);
    let pull = node(&b, &gr, "#11");
    assert!(pull.outside);
    assert!(pull.x < node(&b, &gr, "#10").x);
    assert_eq!(gr.edges.len(), 1);
    // From the pull request, the same two.
    let from_pull = graph::build(&b, b.pull(0, 11).unwrap(), WIDE);
    assert_eq!(ids_of(&b, &from_pull), ["#10", "#11"]);
}

/// library-zvi as it stood on 2026-10-21: its children in four stores, in
/// four waves with three in the widest; `six_waves` is 2026-10-22's, its
/// last two waves its own children.
fn zvi(six_waves: bool) -> Board {
    let kid = |id: &str| g(id).parent("library-zvi").status("closed");
    let mut lib = vec![g("library-zvi").status("in_progress").epic()];
    if six_waves {
        lib.push(kid("library-zvi.1").blocked_by(&["ctx-2bn"]));
        lib.push(
            kid("library-zvi.2")
                .status("open")
                .blocked_by(&["library-zvi.1"]),
        );
    } else {
        lib.push(kid("library-zvi.1").status("open").blocked_by(&["ctx-2bn"]));
    }
    board_of(vec![
        store("/srv/data/context", lib),
        store(
            "/srv/example-store",
            vec![
                kid("ctx-5t7"),
                kid("ctx-xvl").blocked_by(&["ctx-5t7"]),
                kid("ctx-v8m").blocked_by(&["ctx-xvl"]),
                kid("ctx-2bn").blocked_by(&["ctx-v8m", "desk-cih", "srv-lbpn"]),
            ],
        ),
        store("/srv/desk", vec![kid("desk-cih").blocked_by(&["ctx-5t7"])]),
        store("/srv", vec![kid("srv-lbpn").blocked_by(&["ctx-5t7"])]),
    ])
}

#[test]
fn runs_down_the_pane_when_that_shows_the_whole_family_readable_and_across_would_not() {
    let b = zvi(false);
    let pane = (600., 300.);
    let gr = graph::build(&b, row(&b, "library-zvi"), pane);
    assert_eq!(gr.orientation, Orientation::Down);
    assert!(graph::fit_scale(pane, gr.width, gr.height) >= READABLE_SCALE);
    let y = |id| node(&b, &gr, id).y;
    assert!(y("ctx-5t7") < y("ctx-xvl"));
    assert!(y("ctx-xvl") < y("ctx-v8m"));
    assert!(y("ctx-v8m") < y("ctx-2bn"));
    let (from, to) = (node(&b, &gr, "ctx-5t7"), node(&b, &gr, "desk-cih"));
    let path = graph::edge_path(from, to, Orientation::Down);
    assert_eq!(path[0], (from.x + NODE_WIDTH / 2., from.y + NODE_HEIGHT));
    assert_eq!(path[3], (to.x + NODE_WIDTH / 2., to.y));
}

#[test]
fn stays_across_in_a_wide_pane() {
    let b = zvi(false);
    let gr = graph::build(&b, row(&b, "library-zvi"), (1400., 300.));
    assert_eq!(gr.orientation, Orientation::Across);
}

fn whole(gr: &graph::Graph, pane: (f32, f32)) {
    assert!(gr.width * gr.scale <= pane.0 + 0.01);
    assert!(gr.height * gr.scale <= pane.1 + 0.01);
    for n in &gr.nodes {
        assert!(n.x + NODE_WIDTH <= gr.width);
        assert!(n.y + NODE_HEIGHT <= gr.height);
    }
}

#[test]
fn wraps_into_bands_one_under_the_next_to_show_the_whole_chain_at_full_size() {
    let b = zvi(true);
    let pane = (900., 300.);
    let gr = graph::build(&b, row(&b, "library-zvi"), pane);
    assert_eq!(gr.orientation, Orientation::Across);
    assert_eq!(gr.scale, 1.);
    whole(&gr, pane);
    let band = node(&b, &gr, "ctx-2bn").y;
    let tops: std::collections::BTreeSet<bool> = gr.nodes.iter().map(|n| n.y >= band).collect();
    assert_eq!(tops.len(), 2);
    let (from, to) = (node(&b, &gr, "ctx-v8m"), node(&b, &gr, "ctx-2bn"));
    assert!(to.y > from.y + NODE_HEIGHT);
    assert!(to.x < from.x);
    let path = graph::edge_path(from, to, gr.orientation);
    assert_eq!(path[0], (from.x + NODE_WIDTH / 2., from.y + NODE_HEIGHT));
}

#[test]
fn shrinks_a_little_below_readable_rather_than_hide_part_of_a_small_family() {
    let b = zvi(true);
    let pane = (400., 230.);
    let gr = graph::build(&b, row(&b, "library-zvi"), pane);
    assert_eq!(gr.nodes.len(), 8);
    assert!(gr.scale >= WHOLE_SCALE);
    assert!(gr.scale < READABLE_SCALE);
    whole(&gr, pane);
}

#[test]
fn opens_readable_and_pans_when_even_that_would_not_show_it_whole() {
    let b = zvi(true);
    let gr = graph::build(&b, row(&b, "library-zvi"), (300., 160.));
    assert_eq!(gr.scale, READABLE_SCALE);
    assert_eq!(gr.orientation, Orientation::Across);
}

fn big() -> Board {
    let mut items = vec![b("aqg-0gy").epic()];
    for i in 0..116 {
        let mut kid = b(&format!("aqg-0gy.{}", i + 1)).parent("aqg-0gy");
        if i % 3 == 0 {
            kid = kid.status("closed");
        }
        if i >= 100 {
            kid = kid.blocked_by(&[&format!("aqg-0gy.{}", i - 99)]);
        }
        items.push(kid);
    }
    board_of(vec![store("/games/agentquest", items)])
}

#[test]
fn wraps_its_columns_so_a_hundred_children_stay_readable_at_full_size_in_a_pane() {
    let b = big();
    let gr = graph::build(&b, 0, (900., 640.));
    assert_eq!(gr.orientation, Orientation::Across);
    assert_eq!(gr.nodes.len(), 116);
    let mut seen = std::collections::HashSet::new();
    for n in &gr.nodes {
        assert!(n.x + NODE_WIDTH <= gr.width);
        assert!(n.y + NODE_HEIGHT <= gr.height);
        assert!(seen.insert((n.x as i32, n.y as i32)));
    }
    assert_eq!(gr.scale, READABLE_SCALE);
    assert!(gr.height * READABLE_SCALE <= 640.);
    assert!(gr.width <= 11. * (NODE_WIDTH + 60.));
}

#[test]
fn puts_unfinished_work_first_in_each_column() {
    let b = big();
    let gr = graph::build(&b, 0, (900., 640.));
    let mut first: Vec<&graph::Node> = gr.nodes.iter().filter(|n| n.x == gr.nodes[0].x).collect();
    first.sort_by(|a, b| a.y.total_cmp(&b.y));
    let closed = |n: &graph::Node| b.rows[n.row].status == Status::Closed;
    let at = first.iter().position(|n| closed(n)).unwrap();
    assert!(at > 0);
    assert!(first[at..].iter().all(|n| closed(n)));
}

#[test]
fn gives_each_project_in_one_graph_its_own_shelf_the_same_one_every_time() {
    let first = graph::project_colours(&["library", "ctx", "srv", "desk"], 10);
    let again = graph::project_colours(&["desk", "srv", "ctx", "library"], 10);
    assert_eq!(first, again);
    let shelves: std::collections::HashSet<usize> = first.values().copied().collect();
    assert_eq!(shelves.len(), 4);
    assert!(first.values().all(|&c| c < 10));
}

/// BeadsColumn.test.tsx: stores by label, claimed work before ready, each
/// newest first; a quiet store left out, a failed one listed with the work
/// it last read, a repository not at all.
#[test]
fn the_column_groups_stores_claimed_before_ready_newest_first() {
    let mut failed = store("/srv/f", vec![b("f-1")]);
    if let Project::Beads(s) = &mut failed {
        s.error = Some("permission denied".into());
    }
    let b = board_of(vec![
        store(
            "/srv/zeta",
            vec![
                b("z-1").status("in_progress").updated("2026-10-02"),
                b("z-2").status("in_progress").updated("2026-10-09"),
                b("z-3").updated("2026-10-03"),
                b("z-4").updated("2026-10-08"),
                b("z-5").status("closed"),
                b("z-6").status("deferred"),
                b("z-7").blocked_by(&["z-3"]),
            ],
        ),
        store("/srv/alpha", vec![b("a-1")]),
        store("/srv/quiet", vec![b("q-1").status("closed")]),
        repo(),
        failed,
    ]);
    let m = column::moving(&b);
    let ids =
        |rows: &[usize]| -> Vec<String> { rows.iter().map(|&i| b.rows[i].id.clone()).collect() };
    let labels: Vec<&str> = m
        .groups
        .iter()
        .map(|g| b.projects[g.project].label.as_str())
        .collect();
    assert_eq!(labels, ["a", "f", "z"]);
    assert_eq!(ids(&m.groups[2].in_progress), ["z-2", "z-1"]);
    assert_eq!(ids(&m.groups[2].ready), ["z-4", "z-3"]);
    assert_eq!(ids(&m.groups[0].ready), ["a-1"]);
    assert_eq!(m.failed, [4], "a failed store is listed, not quiet");
}
