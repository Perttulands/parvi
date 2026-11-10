//! The node's real list of areas holds together: no two share a name, a
//! frame kind or a command, none takes a reserved kind or one of the
//! binary's own verbs, and `parvi kinds` prints who owns which kinds. All
//! of it is read from the listings; no area is built.

use std::process::Command;

use parvi_node_kit::listing;

#[test]
fn the_listed_areas_do_not_conflict() {
    listing::check(parvi::AREAS).unwrap();
    for area in parvi::AREAS {
        for command in area.commands {
            let noun = command.split('_').next().unwrap();
            assert!(
                !parvi::VERBS.contains(&noun),
                "{}'s command {command} takes the binary's verb {noun}",
                area.name
            );
        }
    }
}

#[test]
fn parvi_kinds_prints_the_table_of_the_listed_areas() {
    let out = Command::new(env!("CARGO_BIN_EXE_parvi"))
        .arg("kinds")
        .output()
        .unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    assert_eq!(printed, listing::kinds_table(parvi::AREAS));
    for area in parvi::AREAS.iter().filter(|a| !a.kinds.is_empty()) {
        assert!(
            printed
                .lines()
                .any(|l| l.ends_with(&format!("  {}", area.name))),
            "{} has no row in\n{printed}",
            area.name
        );
    }
    assert!(printed.contains("the node's hello"), "{printed}");
}
