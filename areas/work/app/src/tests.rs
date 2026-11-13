//! The Work page as the operator drives it: one project's open work as a
//! tree, the keys, a search across every store, choosing the project, an id
//! in the text and Back, a view command's item, and a new snapshot keeping
//! what the operator was doing. The page runs in a test window with no
//! shell; every snapshot it gets is one a test hands it.

use super::{Entry, Work};
use crate::store::work::prose::BEAD_LINK;
use crate::store::work::tests::{Bead, b, repo, store};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use parvi_terminals_contract::{FocusedSession, Session, SessionList};
use parvi_work_contract::{ProjectView as Project, RepoView as Repo, Snapshot, StoreView as Store};
use std::sync::Arc;

/// Two stores linked across, a repository, and one that failed.
fn snapshot() -> Snapshot {
    Snapshot {
        refreshed: 0,
        projects: vec![
            store(
                "/srv/data/context",
                vec![
                    b("library-zvi").title("Grill brain ownership").epic(),
                    b("library-zvi.1")
                        .title("Move the remaining records")
                        .parent("library-zvi")
                        .blocked_by(&["ctx-xvl"])
                        .body("The text of library-zvi.1; it waits on ctx-xvl."),
                    b("library-zvi.2")
                        .title("Project homes for every project")
                        .parent("library-zvi")
                        .blocked_by(&["library-zvi.1"]),
                ],
            ),
            store(
                "/srv/other-store",
                vec![
                    b("ctx-xvl")
                        .title("Connector and Ask cite the evidence")
                        .status("closed")
                        .closed("2026-10-20"),
                    b("ctx-old").title("An old question"),
                ],
            ),
            repo(),
            Project::Github(Repo {
                name: "o/broken".into(),
                error: Some("gh: not logged in".into()),
                ..Repo::default()
            }),
        ],
    }
}

fn page(cx: &mut TestAppContext) -> (Entity<Work>, &mut VisualTestContext) {
    cx.update(|cx| {
        parvi_app_shell::prepare(cx);
    });
    let (work, cx) = cx.add_window_view(|window, cx| {
        let (tx, _) = async_channel::unbounded();
        let hub = cx
            .new(|_| parvi_app_kit::Hub::new(parvi_app_kit::link::socket::Net::start(), tx, None));
        Work::new(parvi_app_kit::Host::new(&hub), window, cx)
    });
    cx.update(|window, cx| {
        work.update(cx, |w, cx| {
            w.project = Some("/srv/data/context".into());
            w.snapshots.insert("node".into(), Arc::new(snapshot()));
            w.rebuild(cx);
            window.focus(&w.focus, cx);
        })
    });
    cx.run_until_parked();
    (work, cx)
}

/// The list as (id, depth), labels as their words.
fn listed(work: &Entity<Work>, cx: &mut VisualTestContext) -> Vec<(String, usize)> {
    work.read_with(cx, |w, _| {
        w.entries
            .iter()
            .map(|e| match e {
                Entry::Item { row, depth, .. } => (w.board.rows[*row].id.clone(), *depth),
                Entry::Label(l) => (l.to_string(), 0),
                Entry::More(n) => (format!("{n} more"), 0),
            })
            .collect()
    })
}

fn shown(work: &Entity<Work>, cx: &mut VisualTestContext) -> Option<String> {
    work.read_with(cx, |w, _| w.shown_row().map(|i| w.board.rows[i].id.clone()))
}

fn select(work: &Entity<Work>, cx: &mut VisualTestContext, id: &str) {
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let row = w.board.rows.iter().position(|r| r.id == id).unwrap();
            w.select(row, cx);
        })
    });
}

fn tree() -> Vec<(String, usize)> {
    vec![
        ("library-zvi".into(), 0),
        ("library-zvi.1".into(), 1),
        ("library-zvi.2".into(), 1),
    ]
}

#[gpui::test]
fn opens_on_the_projects_open_work_as_a_tree_with_the_first_item_shown(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    assert_eq!(listed(&work, cx), tree());
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi"));
    // Drawn: the item's title heads the detail.
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("work-card-title-Grill brain ownership")
            .is_some()
    );
    assert!(cx.debug_bounds("work-row-library-zvi.2").is_some());
    assert!(
        cx.debug_bounds("work-row-ctx-old").is_none(),
        "another project's work waits for a search"
    );
}

#[gpui::test]
fn the_keys_walk_the_tree_and_slash_searches_every_store(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.simulate_keystrokes("j");
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi.1"));
    cx.simulate_keystrokes("down");
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi.2"));
    cx.simulate_keystrokes("j");
    assert_eq!(
        shown(&work, cx).as_deref(),
        Some("library-zvi.2"),
        "the walk stops at the end"
    );
    cx.simulate_keystrokes("k up");
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi"));

    // `/` puts the keys in the search; it answers as it is typed, this
    // project's first, finished work too.
    cx.simulate_keystrokes("/");
    cx.simulate_input("evidence");
    assert_eq!(
        listed(&work, cx),
        [("Other projects".to_string(), 0), ("ctx-xvl".into(), 0)]
    );
    assert_eq!(shown(&work, cx).as_deref(), Some("ctx-xvl"));
    // Escape clears it, and the tree is back.
    cx.simulate_keystrokes("escape");
    assert_eq!(listed(&work, cx), tree());
}

#[gpui::test]
fn p_chooses_the_project_by_its_name_and_the_choice_stays(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.simulate_keystrokes("p");
    assert!(work.read_with(cx, |w, _| w.picking));
    cx.simulate_input("other");
    let names = work.read_with(cx, |w, cx| {
        w.projects_listed(cx)
            .into_iter()
            .map(|p| w.board.projects[p].path.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(names, ["/srv/other-store"]);
    cx.simulate_keystrokes("enter");
    work.read_with(cx, |w, _| {
        assert!(!w.picking);
        assert_eq!(w.project.as_deref(), Some("/srv/other-store"));
        assert_eq!(w.query, "", "the name typed was not a search");
    });
    assert_eq!(listed(&work, cx), [("ctx-old".to_string(), 0)]);
    // Escape leaves a choice unmade.
    cx.simulate_keystrokes("p");
    cx.simulate_input("context");
    cx.simulate_keystrokes("escape");
    work.read_with(cx, |w, _| {
        assert!(!w.picking);
        assert_eq!(w.project.as_deref(), Some("/srv/other-store"));
    });
}

#[gpui::test]
fn with_no_choice_the_focused_sessions_folder_names_the_project(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.project = None;
            w.sessions = Some(cx.new(|_| SessionList {
                sessions: vec![Session {
                    node: "node".into(),
                    name: "shell".into(),
                    cwd: Some("/srv/other-store/src".into()),
                    ..Session::default()
                }],
                focused: Some(FocusedSession {
                    node: "node".into(),
                    user: String::new(),
                    name: "shell".into(),
                }),
            }));
            w.relist(cx);
        })
    });
    assert_eq!(listed(&work, cx), [("ctx-old".to_string(), 0)]);
}

#[gpui::test]
fn an_id_in_the_text_shows_that_item_and_back_returns(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    select(&work, cx, "library-zvi.1");
    let trail =
        |work: &Entity<Work>, cx: &mut VisualTestContext| work.read_with(cx, |w, _| w.trail.len());
    assert_eq!(
        trail(&work, cx),
        0,
        "nothing behind a row chosen in the list"
    );
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let project = w.board.rows[w.selected_row().unwrap()].project;
            w.follow_link(&format!("{BEAD_LINK}ctx-xvl"), project, cx);
            // A family row is followed the same way.
            w.follow("bead:n:/srv/data/context#library-zvi.2", cx);
        })
    });
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi.2"));
    assert_eq!(trail(&work, cx), 2);
    cx.simulate_keystrokes("alt-left");
    assert_eq!(shown(&work, cx).as_deref(), Some("ctx-xvl"));
    cx.update(|_, cx| work.update(cx, |w, cx| w.back(cx)));
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi.1"));
    cx.simulate_keystrokes("alt-left");
    assert_eq!(
        shown(&work, cx).as_deref(),
        Some("library-zvi.1"),
        "nothing behind"
    );
    // `#10` in an issue's text shows that issue.
    select(&work, cx, "#11");
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let project = w.board.rows[w.selected_row().unwrap()].project;
            w.follow_link("#10", project, cx)
        })
    });
    assert_eq!(shown(&work, cx).as_deref(), Some("#10"));
}

#[gpui::test]
fn an_item_a_command_asks_for_shows_from_any_project(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.wanted = Some(("bead:n:/srv/other-store#ctx-old".into(), None));
            w.open_wanted(cx);
        })
    });
    assert_eq!(shown(&work, cx).as_deref(), Some("ctx-old"));
    assert_eq!(listed(&work, cx), tree(), "the project stays");
}

#[gpui::test]
fn a_new_snapshot_keeps_the_search_and_the_item_shown(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.simulate_keystrokes("/");
    cx.simulate_input("zvi");
    select(&work, cx, "library-zvi.1");
    let mut next = snapshot();
    if let Project::Beads(s) = &mut next.projects[0] {
        let items = s.items.as_mut().unwrap();
        items.push(b("library-zvi.3").parent("library-zvi"));
    }
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.snapshots.insert("node".into(), Arc::new(next));
            w.rebuild(cx);
        })
    });
    assert_eq!(shown(&work, cx).as_deref(), Some("library-zvi.1"));
    work.read_with(cx, |w, _| assert_eq!(w.query, "zvi"));
    assert_eq!(listed(&work, cx).len(), 4);
}

#[gpui::test]
fn a_snapshot_frame_is_read_off_the_window_and_lists_when_it_lands(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    let mut next = snapshot();
    if let Project::Beads(s) = &mut next.projects[0] {
        s.items
            .as_mut()
            .unwrap()
            .push(b("library-new").title("Arrived"));
    }
    let body = serde_json::to_vec(&next).unwrap();
    cx.update(|window, cx| {
        work.update(cx, |w, cx| {
            parvi_app_kit::Page::frame(
                w,
                &parvi_core::NodeId("node".into()),
                parvi_work_contract::SNAPSHOT,
                &body,
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    assert!(listed(&work, cx).contains(&("library-new".to_string(), 0)));
}

#[gpui::test]
fn the_item_menu_copies_its_id(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|window, cx| {
        work.update(cx, |w, cx| {
            let row = w.board.rows.iter().position(|r| r.id == "#10").unwrap();
            let menu = w.item_menu(row);
            assert_eq!(
                menu.iter()
                    .take(2)
                    .map(|r| r.label.as_str())
                    .collect::<Vec<_>>(),
                ["Copy id", "Copy id and title"]
            );
            (menu[0].run.as_ref().unwrap())(window, cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("o/r#10")
            );
        });
    });
}

#[gpui::test]
fn reveal_shows_the_named_nodes_copy(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|window, cx| {
        work.update(cx, |w, cx| {
            let mut second = snapshot();
            if let Project::Beads(store) = &mut second.projects[0] {
                store.items.as_mut().unwrap()[0].title = "Other node".into();
            }
            w.snapshots.insert("other".into(), Arc::new(second));
            w.rebuild(cx);
            <Work as parvi_app_kit::Page>::answer(
                w,
                parvi_app_kit::Ask::Reveal {
                    what: "item".into(),
                    id: "bead:n:/srv/data/context#library-zvi".into(),
                    node: Some(parvi_core::NodeId("other".into())),
                    part: None,
                    beside: false,
                },
                "test",
                window,
                cx,
            );
            let row = w.selected_row().unwrap();
            assert_eq!(w.board.rows[row].title, "Other node");
            assert_eq!(w.selected_node.as_deref(), Some("other"));
        })
    });
}

#[gpui::test]
fn says_it_is_reading_while_no_project_has_answered(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let reading = Snapshot {
                refreshed: 0,
                projects: vec![Project::Beads(Store {
                    folder: "/srv/data/context".into(),
                    ..Default::default()
                })],
            };
            w.snapshots.insert("node".into(), Arc::new(reading));
            w.rebuild(cx);
        })
    });
    cx.run_until_parked();
    let label = work.read_with(cx, |w, _| w.board.projects[0].label.clone());
    let words: &'static str = format!("empty-Reading {label}…").leak();
    assert!(cx.debug_bounds(words).is_some());
}

#[test]
fn session_names() {
    use parvi_terminals_contract::session_copy::next_name;
    assert_eq!(super::session_name("example/project", "#42"), "project-42");
    assert_eq!(super::session_name("ctx", "ctx-1a.2"), "ctx-1a-2");
    assert_eq!(
        next_name(&super::session_name("o/r", "#1"), &["r-1", "r-1-2"]),
        "r-1-3"
    );
}
