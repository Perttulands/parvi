//! The workspace layout holds (ARCHITECTURE.md): crates are found by where
//! they sit, so a new area adds no line to the root manifest or CI, and
//! dependencies point one way.

use std::fs;
use std::path::{Path, PathBuf};

use toml::Table;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn manifest(dir: &Path) -> Table {
    let text = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    text.parse().unwrap()
}

/// Each crate directory under `parent`, as (its name there, its path).
fn crates(parent: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .map(|path| (path.file_name().unwrap().to_string_lossy().into(), path))
        .collect();
    found.sort();
    found
}

fn package_name(dir: &Path) -> String {
    manifest(dir)["package"]["name"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The workspace paths a crate's normal and build dependencies point at,
/// relative to the root (`areas/terminals/contract`).
fn uses(dir: &Path) -> Vec<String> {
    uses_in(dir, &["dependencies", "build-dependencies"])
}

fn uses_in(dir: &Path, tables: &[&str]) -> Vec<String> {
    let manifest = manifest(dir);
    let root = fs::canonicalize(root()).unwrap();
    tables
        .iter()
        .filter_map(|table| manifest.get(*table).and_then(|deps| deps.as_table()))
        .flat_map(|deps| deps.values())
        .filter_map(|dep| dep.get("path").and_then(|path| path.as_str()))
        .map(|path| {
            let target = fs::canonicalize(dir.join(path)).unwrap();
            let relative = target.strip_prefix(&root).unwrap();
            relative.to_string_lossy().replace('\\', "/")
        })
        .collect()
}

#[test]
fn members_are_found_by_where_they_sit() {
    let root = manifest(&root());
    let members: Vec<_> = root["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member.as_str().unwrap())
        .collect();
    assert_eq!(members, ["kit/*", "engine/*", "areas/*/*", "bin/*"]);
}

/// CI runs app crates in their own job by name (`parvi-*-app`), so an
/// area's crates are named by their side.
#[test]
fn an_areas_crates_are_its_contract_node_and_app() {
    for (area, dir) in crates_of_areas() {
        for (side, crate_dir) in crates(&dir) {
            let expected = match side.as_str() {
                "contract" => format!("parvi-{area}-contract"),
                "node" => format!("parvi-{area}"),
                "app" => format!("parvi-{area}-app"),
                other => panic!("areas/{area}/{other} is not contract, node or app"),
            };
            assert_eq!(package_name(&crate_dir), expected, "areas/{area}/{side}");
        }
    }
}

fn crates_of_areas() -> Vec<(String, PathBuf)> {
    let mut areas: Vec<_> = fs::read_dir(root().join("areas"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| (path.file_name().unwrap().to_string_lossy().into(), path))
        .collect();
    areas.sort();
    areas
}

/// An engine crate that depends on nothing, or the mission model, which
/// depends only on `core` and serde's data formats; either way a contract
/// using it still does no I/O.
fn is_plain_engine_crate(path: &str) -> bool {
    const MISSION_MODEL_USES: [&str; 4] = ["parvi-core", "serde", "serde_json", "toml"];
    let dir = root().join(path);
    let manifest = manifest(&dir);
    let deps: Vec<&str> = manifest
        .get("dependencies")
        .and_then(|deps| deps.as_table())
        .map(|deps| deps.keys().map(String::as_str).collect())
        .unwrap_or_default();
    match path {
        "engine/mission-model" => deps.iter().all(|dep| MISSION_MODEL_USES.contains(dep)),
        _ => path.starts_with("engine/") && deps.is_empty(),
    }
}

#[test]
fn dependencies_point_one_way() {
    for parent in ["kit", "engine"] {
        for (name, dir) in crates(&root().join(parent)) {
            for used in uses(&dir) {
                assert!(
                    used.starts_with("kit/") || used.starts_with("engine/"),
                    "{parent}/{name} uses {used}: the kit and the engine use no area"
                );
            }
        }
    }
    for (area, dir) in crates_of_areas() {
        for (side, crate_dir) in crates(&dir) {
            for used in uses(&crate_dir) {
                let allowed = match side.as_str() {
                    // Plain data shared below the areas, such as work
                    // references, is an engine crate with no dependencies.
                    "contract" => used == "kit/core" || is_plain_engine_crate(&used),
                    // The GUI toolkit stays out of the node, the web server
                    // out of the app.
                    "node" => used != "kit/app-kit",
                    _ => used != "kit/node-kit",
                } && (used.starts_with("kit/")
                    || used.starts_with("engine/")
                    || used.starts_with("areas/") && used.ends_with("/contract"));
                assert!(
                    allowed,
                    "areas/{area}/{side} uses {used}: an area reaches another \
                     only by its contract or a provided interface"
                );
            }
        }
    }
}

/// An area's tests drive the shell crate, not a binary, so they build no
/// other area's pages.
#[test]
fn an_areas_tests_build_no_binary() {
    for (area, dir) in crates_of_areas() {
        for (side, crate_dir) in crates(&dir) {
            for used in uses_in(&crate_dir, &["dev-dependencies"]) {
                assert!(
                    !used.starts_with("bin/"),
                    "areas/{area}/{side}'s tests use {used}"
                );
            }
        }
    }
}
