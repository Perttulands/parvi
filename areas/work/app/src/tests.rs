//! The Work page as the operator drives it, against CHROTE's
//! `BeadsView.test.tsx`: Beads stores and a GitHub repository in one table,
//! the keys, the graph and text beside a selection, an id in the text, a
//! view command's item, and a new snapshot keeping what the operator was
//! doing. The page runs in a test window with no shell; every snapshot it
//! gets is one a test hands it.

use super::Work;
use crate::store::work::graph;
use crate::store::work::prose::BEAD_LINK;
use crate::store::work::tests::{Bead, b, repo, store};
use crate::store::work::{Column, Status};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use parvi_work_contract::{ProjectView as Project, RepoView as Repo, Snapshot, StoreView as Store};

/// BeadsView.test.tsx's stores, a repository, and one that failed.
fn snapshot() -> Snapshot {
    Snapshot {
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
            w.snapshots.insert("node".into(), snapshot());
            w.rebuild(cx);
            window.focus(&w.focus, cx);
        })
    });
    cx.run_until_parked();
    (work, cx)
}

fn ids(work: &Entity<Work>, cx: &mut VisualTestContext) -> Vec<String> {
    work.read_with(cx, |w, _| {
        w.sliced
            .rows
            .iter()
            .map(|&i| w.board.rows[i].id.clone())
            .collect()
    })
}

fn selected(work: &Entity<Work>, cx: &mut VisualTestContext) -> Option<String> {
    work.read_with(cx, |w, _| {
        w.selected_row().map(|i| w.board.rows[i].id.clone())
    })
}

fn select(work: &Entity<Work>, cx: &mut VisualTestContext, id: &str) {
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let row = w.board.rows.iter().position(|r| r.id == id).unwrap();
            w.select(row, cx);
        })
    });
}

#[gpui::test]
fn a_wide_graph_can_be_panned_with_its_buttons(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    let mut items = vec![b("chain-0").epic()];
    for i in 1..=14 {
        items.push(
            b(&format!("chain-{i}"))
                .parent("chain-0")
                .blocked_by(&[&format!("chain-{}", i - 1)]),
        );
    }
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.snapshots.insert(
                "node".into(),
                Snapshot {
                    projects: vec![store("/chain", items)],
                },
            );
            w.rebuild(cx);
        })
    });
    select(&work, cx, "chain-0");
    cx.run_until_parked();
    let before = work.read_with(cx, |w, _| w.graph_scroll.offset().x);
    let right = cx
        .debug_bounds("work-graph-right")
        .expect("wide graph has a right pan button")
        .center();
    cx.simulate_click(right, gpui::Modifiers::none());
    cx.run_until_parked();
    let after = work.read_with(cx, |w, _| w.graph_scroll.offset().x);
    assert!(after < before, "right button moves the graph");
    let left = cx.debug_bounds("work-graph-left").unwrap().center();
    cx.simulate_click(left, gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(work.read_with(cx, |w, _| w.graph_scroll.offset().x), before);
}

#[gpui::test]
fn draws_every_item_of_every_project_in_one_table_and_says_what_failed(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    // Five Beads, two issues and a pull request.
    assert_eq!(ids(&work, cx).len(), 8);
    work.read_with(cx, |w, _| {
        let p = &w.board.projects;
        assert_eq!(p.iter().filter(|p| p.read).count(), 3);
        assert_eq!(p[3].error.as_deref(), Some("gh: not logged in"));
        assert!(!p[3].read, "a failed read is not an empty one");
        // A blocker in another store that is finished does not block.
        let row = w
            .board
            .rows
            .iter()
            .find(|r| r.id == "library-zvi.1")
            .unwrap();
        assert_eq!(row.status, Status::Open);
    });
}

#[gpui::test]
fn says_it_is_still_reading_while_no_project_has_answered(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let reading = Snapshot {
                projects: vec![
                    Project::Beads(Store {
                        folder: "/srv".into(),
                        ..Default::default()
                    }),
                    Project::Github(Repo {
                        name: "o/r".into(),
                        ..Default::default()
                    }),
                ],
            };
            w.snapshots.insert("node".into(), reading);
            w.rebuild(cx);
        })
    });
    work.read_with(cx, |w, _| {
        assert!(w.sliced.rows.is_empty());
        assert!(
            w.board
                .projects
                .iter()
                .all(|p| !p.read && p.error.is_none())
        );
    });
    // Drawn, it says so: the page renders without a panic at any state.
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("work-loading-Reading projects: 0 of 2 loaded")
            .is_some()
    );
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.snapshots.insert(
                "node".into(),
                Snapshot {
                    projects: vec![
                        store("/one", vec![b("one-1")]),
                        Project::Beads(Store {
                            folder: "/two".into(),
                            ..Store::default()
                        }),
                    ],
                },
            );
            w.rebuild(cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("work-loading-Reading projects: 1 of 2 loaded")
            .is_some()
    );
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.snapshots.insert(
                "node".into(),
                Snapshot {
                    projects: vec![
                        store("/one", vec![b("one-1")]),
                        store("/two", vec![b("two-1")]),
                    ],
                },
            );
            w.rebuild(cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("work-loading-Reading projects: 1 of 2 loaded")
            .is_none()
    );
}

#[gpui::test]
fn the_keys_walk_open_and_let_go_and_slash_searches(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|_, cx| work.update(cx, |w, cx| w.sort_by(Column::Id, cx)));
    let first = ids(&work, cx);
    assert_eq!(selected(&work, cx), None);
    cx.simulate_keystrokes("j");
    assert_eq!(selected(&work, cx).as_deref(), Some(first[0].as_str()));
    // A selection brings its graph and card beside the table.
    assert!(work.read_with(cx, |w, _| w.card));
    cx.simulate_keystrokes("down j");
    assert_eq!(selected(&work, cx).as_deref(), Some(first[2].as_str()));
    cx.simulate_keystrokes("k up");
    assert_eq!(selected(&work, cx).as_deref(), Some(first[0].as_str()));
    cx.simulate_keystrokes("enter");
    assert!(work.read_with(cx, |w, _| w.card));
    // Escape lets the selection go, and the table has the pane again.
    cx.simulate_keystrokes("escape");
    assert_eq!(selected(&work, cx), None);
    assert!(!work.read_with(cx, |w, _| w.card));

    // `/` puts the keys in the search; ids, titles and text answer.
    cx.simulate_keystrokes("/");
    cx.simulate_input("project");
    assert_eq!(ids(&work, cx), ["library-zvi.2"]);
    cx.simulate_keystrokes("backspace backspace backspace backspace backspace backspace backspace");
    cx.simulate_input("evidence");
    // CHROTE asks its server for the text; the snapshot carries it.
    assert_eq!(ids(&work, cx), ["ctx-xvl"]);
    // Down takes the keys back to the table on the first row.
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&work, cx).as_deref(), Some("ctx-xvl"));
    // Escape lets the row go, then clears the search.
    cx.simulate_keystrokes("escape");
    assert_eq!(selected(&work, cx), None);
    cx.simulate_keystrokes("escape");
    assert_eq!(ids(&work, cx).len(), 8);
}

#[gpui::test]
fn keeps_one_project_by_its_name_and_sorts_by_size(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    work.update(cx, |w, _| {
        w.slice.only_project("/srv/data/context");
        w.reslice();
    });
    let mut kept = ids(&work, cx);
    kept.sort();
    assert_eq!(kept, ["library-zvi", "library-zvi.1", "library-zvi.2"]);
    cx.update(|_, cx| work.update(cx, |w, cx| w.sort_by(Column::Size, cx)));
    assert_eq!(ids(&work, cx)[0], "library-zvi.1", "the only one with text");
    work.update(cx, |w, _| {
        w.slice.only_project("/srv/data/context");
        w.reslice();
    });
    assert_eq!(ids(&work, cx).len(), 8);
}

#[gpui::test]
fn the_graph_follows_the_selected_row_and_names_it_to_an_agent(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    select(&work, cx, "library-zvi.2");
    work.read_with(cx, |w, _| {
        let row = w.selected_row().unwrap();
        let g = graph::build(&w.board, row, (500., 270.));
        let drawn: Vec<&str> = g
            .nodes
            .iter()
            .map(|n| w.board.rows[n.row].id.as_str())
            .collect();
        assert_eq!(w.board.rows[g.head].id, "library-zvi");
        assert!(drawn.contains(&"library-zvi.2"));
        assert!(drawn.contains(&"ctx-xvl"), "its blocker from another store");
        assert_eq!(
            w.board.rows[row].line(&w.board),
            "bead library-zvi.2: Project homes for every project"
        );
    });
    // A node selects its row, and the drawing stays the same family.
    select(&work, cx, "library-zvi.1");
    work.read_with(cx, |w, _| {
        let g = graph::build(&w.board, w.selected_row().unwrap(), (500., 270.));
        assert_eq!(w.board.rows[g.head].id, "library-zvi");
    });
    // An issue's graph holds the pull request that closes it.
    select(&work, cx, "#10");
    work.read_with(cx, |w, _| {
        let g = graph::build(&w.board, w.selected_row().unwrap(), (500., 270.));
        let mut drawn: Vec<&str> = g
            .nodes
            .iter()
            .map(|n| w.board.rows[n.row].id.as_str())
            .collect();
        drawn.sort();
        assert_eq!(drawn, ["#10", "#11"]);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn an_id_in_the_text_selects_that_item(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    select(&work, cx, "library-zvi.1");
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let project = w.board.rows[w.selected_row().unwrap()].project;
            w.follow_link(&format!("{BEAD_LINK}ctx-xvl"), project, cx)
        })
    });
    assert_eq!(selected(&work, cx).as_deref(), Some("ctx-xvl"));
    // `#10` in an issue's text opens that issue.
    select(&work, cx, "#11");
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let project = w.board.rows[w.selected_row().unwrap()].project;
            w.follow_link("#10", project, cx)
        })
    });
    assert_eq!(selected(&work, cx).as_deref(), Some("#10"));
}

/// BeadCard.test.tsx "follows an id inside the text and comes back", two
/// ids deep, with Alt+Left and the Back control.
#[gpui::test]
fn back_returns_through_two_followed_ids(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    select(&work, cx, "library-zvi.1");
    let trail =
        |work: &Entity<Work>, cx: &mut VisualTestContext| work.read_with(cx, |w, _| w.trail.len());
    assert_eq!(
        trail(&work, cx),
        0,
        "nothing behind a row chosen in the table"
    );
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            let project = w.board.rows[w.selected_row().unwrap()].project;
            w.follow_link(&format!("{BEAD_LINK}ctx-xvl"), project, cx);
            // A family link on the card is followed the same way.
            w.follow("bead:n:/srv/data/context#library-zvi.2", cx);
        })
    });
    assert_eq!(selected(&work, cx).as_deref(), Some("library-zvi.2"));
    assert_eq!(trail(&work, cx), 2);
    cx.simulate_keystrokes("alt-left");
    assert_eq!(selected(&work, cx).as_deref(), Some("ctx-xvl"));
    cx.update(|_, cx| work.update(cx, |w, cx| w.back(cx)));
    assert_eq!(selected(&work, cx).as_deref(), Some("library-zvi.1"));
    assert!(work.read_with(cx, |w, _| w.card));
    // Nothing behind: Back stays.
    cx.simulate_keystrokes("alt-left");
    assert_eq!(selected(&work, cx).as_deref(), Some("library-zvi.1"));

    // Following the item in hand adds nothing; a row chosen anew starts again.
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.follow("bead:n:/srv/data/context#library-zvi.2", cx);
            w.follow("bead:n:/srv/data/context#library-zvi.2", cx);
        })
    });
    assert_eq!(trail(&work, cx), 1);
    cx.simulate_keystrokes("j");
    assert_eq!(trail(&work, cx), 0);
}

#[gpui::test]
fn shows_an_item_a_command_asked_for_clearing_only_a_slice_that_hides_it(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    work.update(cx, |w, _| {
        w.slice.query = "grill".into();
        w.slice.only_status(Status::Open);
        w.reslice();
    });
    assert_eq!(ids(&work, cx), ["library-zvi"]);
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.wanted = Some(("bead:n:/srv/other-store#ctx-old".into(), None));
            w.open_wanted(cx);
        })
    });
    assert_eq!(selected(&work, cx).as_deref(), Some("ctx-old"));
    work.read_with(cx, |w, _| {
        assert_eq!(w.slice.query, "", "the search hid it");
        assert!(w.slice.statuses.is_some(), "its status was kept: it stays");
        assert!(w.card);
    });
}

#[gpui::test]
fn a_new_snapshot_keeps_the_query_the_selection_the_card_and_the_range(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    select(&work, cx, "library-zvi.1");
    work.update(cx, |w, _| {
        w.slice.query = "zvi".into();
        w.slice.range = Some((w.board.days.first, w.board.days.today));
        w.reslice();
    });
    let mut next = snapshot();
    if let Project::Beads(s) = &mut next.projects[0] {
        let items = s.items.as_mut().unwrap();
        items.push(b("library-zvi.3").parent("library-zvi"));
    }
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.snapshots.insert("node".into(), next);
            w.rebuild(cx);
        })
    });
    assert_eq!(selected(&work, cx).as_deref(), Some("library-zvi.1"));
    work.read_with(cx, |w, _| {
        assert!(w.card);
        assert_eq!(w.slice.query, "zvi");
        assert_eq!(
            w.slice.range,
            Some((w.board.days.first, w.board.days.today))
        );
        assert_eq!(w.sliced.rows.len(), 4);
    });
}

#[gpui::test]
fn a_drag_across_the_strip_keeps_those_days(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    let (first, today) = work.read_with(cx, |w, _| (w.board.days.first, w.board.days.today));
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.drag_to(today, cx);
            w.drag_to(today - 3, cx);
        })
    });
    work.read_with(cx, |w, _| {
        assert_eq!(w.slice.range, Some((today - 3, today)));
        assert!(w.sliced.rows.len() < 8);
        // The strip stays whole.
        assert_eq!(w.sliced.open.len(), (today - first + 1) as usize);
    });
    cx.update(|_, cx| work.update(cx, |w, cx| w.clear_range(cx)));
    assert_eq!(ids(&work, cx).len(), 8);
}

#[gpui::test]
fn a_rows_menu_copies_its_id(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|window, cx| {
        work.update(cx, |w, cx| {
            let row = w.board.rows.iter().position(|r| r.id == "#10").unwrap();
            let menu = w.row_menu(row);
            assert_eq!(
                menu.iter()
                    .filter(|r| !r.is_rule())
                    .map(|r| r.label.as_str())
                    .collect::<Vec<_>>(),
                ["Send…", "Copy id", "Copy id and title"]
            );
            (menu[2].run.as_ref().unwrap())(window, cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("o/r#10")
            );
        });
    });
}

#[gpui::test]
fn reveal_selects_the_named_nodes_copy(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|window, cx| {
        work.update(cx, |w, cx| {
            let mut second = snapshot();
            if let Project::Beads(store) = &mut second.projects[0] {
                store.items.as_mut().unwrap()[0].title = "Other node".into();
            }
            w.snapshots.insert("other".into(), second);
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
fn a_column_row_opens_its_item(cx: &mut TestAppContext) {
    let (work, cx) = page(cx);
    cx.update(|_, cx| {
        work.update(cx, |w, cx| {
            w.slice.only_project("/srv/data/context");
            w.reslice();
            let moving = crate::store::work::column::moving(&w.board);
            let group = moving
                .groups
                .iter()
                .find(|g| w.board.projects[g.project].label == "ctx")
                .unwrap();
            w.open_from_column(group.ready[0], cx);
        })
    });
    assert_eq!(selected(&work, cx).as_deref(), Some("ctx-old"));
    assert!(work.read_with(cx, |w, _| w.card));
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

#[test]
fn folders_written_two_ways_match() {
    assert!(super::same_folder("/home/example/project/", "~/project"));
    assert!(super::same_folder("/srv/x", "/srv/x/"));
    assert!(!super::same_folder("/srv/x", "/srv/y"));
}
