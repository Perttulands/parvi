//! The state directory: strict reads, broken links, selectors, notes and
//! layouts.

use std::path::PathBuf;

use parvi_mission_engine::{Error, Store};
use parvi_mission_model::Mission;

const HELLO: &str = r#"schema = 1
id = "msn_hello"
slug = "hello"
title = "Hello"
rev = 1

[[inputCard]]
id = "inp_hello"
title = "Hello"
goal = "Return the supplied input"

[[formation]]
id = "fmn_work"
type = "solo"
title = "Work"
device = "rig-wsl"
[formation.brief]
goal = "Return a short result using the supplied output contract."
[[formation.input]]
id = "port_in"
label = "Input"
[[formation.output]]
id = "port_out"
label = "Result"
[[formation.slot]]
id = "slot_work"
label = "Worker"
agentId = "builder"
harness = "openai-codex"
model = "" # harness default
effort = "medium"

[[end]]
id = "end_done"
title = "Done"
outcome = "done"

[[connection]]
id = "edge_start"
from = "inp_hello:out"
to = "fmn_work:port_in"

[[connection]]
id = "edge_done"
from = "fmn_work:port_out"
to = "end_done:in"
"#;

/// A scratch state directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("parvi-64-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["missions", "notes", "layout"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        Scratch(dir)
    }

    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.0.join(path), text).unwrap();
    }

    fn store(&self) -> Store {
        Store::new(&self.0)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn code(result: Result<Mission, Error>) -> String {
    result.unwrap_err().code
}

#[test]
fn the_contracts_minimal_mission_loads_with_a_device() {
    let mission = Mission::parse(HELLO).unwrap();
    assert_eq!(mission.formations[0].device, "rig-wsl");
    assert_eq!(mission.formations[0].slots[0].agent_id, "builder");
    assert_eq!(mission.node_name("fmn_work"), "Work");
}

#[test]
fn a_mission_is_read_only_as_strict_toml_of_schema_one() {
    let repeated = HELLO.replace("rev = 1", "rev = 1\nrev = 2");
    assert_eq!(code(Mission::parse(&repeated)), "invalid_definition_source");
    let typo = HELLO.replace("effort = \"medium\"", "efort = \"medium\"");
    assert_eq!(code(Mission::parse(&typo)), "invalid_definition_source");
    let legacy_brief = HELLO.replace("goal = \"Return a short result using the supplied output contract.\"", "goal = \"Return a short result using the supplied output contract.\"\nbeadId = \"example-1\"");
    assert_eq!(
        code(Mission::parse(&legacy_brief)),
        "invalid_definition_source"
    );
    let tool = format!("{HELLO}\n[[tool]]\nid = \"tool_json\"\n");
    assert_eq!(code(Mission::parse(&tool)), "invalid_definition_source");
    let wrong_type = HELLO.replace("rev = 1", "rev = \"1\"");
    assert_eq!(
        code(Mission::parse(&wrong_type)),
        "invalid_definition_source"
    );
    assert_eq!(
        code(Mission::parse(&HELLO.replace("schema = 1", "schema = 2"))),
        "unsupported_schema"
    );
    assert_eq!(
        code(Mission::parse(&HELLO.replace("schema = 1\n", ""))),
        "unsupported_schema"
    );
}

#[test]
fn one_unreadable_mission_never_hides_the_rest() {
    let scratch = Scratch::new("broken");
    scratch.write("missions/hello.mission.toml", HELLO);
    scratch.write("missions/bad.mission.toml", "schema = 1\nschema = 1\n");
    scratch.write("missions/README.md", "not a mission");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "/nonexistent/parvi-64/moved.mission.toml",
        scratch.0.join("missions/moved.mission.toml"),
    )
    .unwrap();
    let store = scratch.store();
    let listings = store.list_missions().unwrap();
    let slugs: Vec<_> = listings
        .iter()
        .map(|listing| listing.slug.as_str())
        .collect();
    #[cfg(unix)]
    assert_eq!(slugs, ["bad", "hello", "moved"]);
    #[cfg(not(unix))]
    assert_eq!(slugs, ["bad", "hello"]);
    assert!(
        listings[0]
            .broken
            .as_deref()
            .unwrap()
            .starts_with("invalid_definition_source")
    );
    assert_eq!(
        (listings[1].id.as_str(), listings[1].broken.as_deref()),
        ("msn_hello", None)
    );
    #[cfg(unix)]
    {
        let words = format!(
            "{} is a symlink to /nonexistent/parvi-64/moved.mission.toml, which does not exist",
            scratch.0.join("missions/moved.mission.toml").display()
        );
        assert_eq!(listings[2].broken.as_deref(), Some(words.as_str()));
        let err = store.read_mission("moved").unwrap_err();
        assert_eq!((err.code.as_str(), err.message), ("broken_link", words));
    }
}

#[test]
fn a_symlinked_mission_is_read_through_its_link() {
    let scratch = Scratch::new("linked");
    scratch.write("hello.toml", HELLO);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            scratch.0.join("hello.toml"),
            scratch.0.join("missions/hello.mission.toml"),
        )
        .unwrap();
        assert_eq!(
            scratch.store().read_mission("hello").unwrap().id,
            "msn_hello"
        );
    }
}

#[test]
fn a_selector_is_a_slug_or_an_id() {
    let scratch = Scratch::new("selector");
    scratch.write("missions/hello.mission.toml", HELLO);
    scratch.write(
        "missions/copy.mission.toml",
        &HELLO.replace("slug = \"hello\"", "slug = \"copy\""),
    );
    let store = scratch.store();
    assert_eq!(store.resolve("hello").unwrap(), "hello");
    assert_eq!(
        store.resolve("msn_hello").unwrap_err().code,
        "ambiguous_selector"
    );
    assert_eq!(store.resolve("nope").unwrap_err().code, "not_found");
    assert_eq!(store.resolve("../hello").unwrap_err().code, "invalid_name");
}

#[test]
fn notes_and_layout_belong_to_their_mission() {
    let scratch = Scratch::new("notes");
    scratch.write("missions/hello.mission.toml", HELLO);
    let store = scratch.store();
    assert!(store.read_notes("hello").unwrap().entries.is_empty());
    assert_eq!(store.read_layout("hello").unwrap(), None);
    let notes = "schema = 2\nmissionId = \"msn_hello\"\nrev = 1\nupdatedAt = \"2026-11-05T00:00:00Z\"\n\n[[entry]]\nid = \"nte_1\"\ntarget = \"mission\"\nauthor = \"human:ui\"\ncreatedAt = \"2026-11-05T00:00:00Z\"\ntext = \"Keep it short.\"\n";
    scratch.write("notes/hello.notes.toml", notes);
    assert_eq!(
        store.read_notes("hello").unwrap().thread("mission").count(),
        1
    );
    scratch.write(
        "notes/hello.notes.toml",
        &notes.replace("msn_hello", "msn_other"),
    );
    assert!(store.read_notes("hello").unwrap().entries.is_empty());
    scratch.write("layout/hello.layout.toml", "schema = 1\nmissionId = \"msn_hello\"\nmissionRev = 1\n[[node]]\nid = \"fmn_work\"\nx = 10\ny = 20\n");
    assert_eq!(store.read_layout("hello").unwrap().unwrap().nodes[0].x, 10);
}

#[test]
fn archons_delivery_notes_load() {
    let parity = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let notes = Store::new(parity).read_notes("delivery").unwrap();
    assert_eq!(notes.entries.len(), 8);
    assert_eq!(notes.thread("fmn_beads").count(), 1);
}

#[cfg(windows)]
#[test]
fn windows_default_root_prefers_localappdata_over_userprofile() {
    const MARKER: &str = "PARVI_86_ROOT_TEST";
    if std::env::var_os(MARKER).is_some() {
        assert_eq!(
            Store::default_root().unwrap(),
            PathBuf::from(r"C:\parvi-local\parvi\missions")
        );
    } else {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows_default_root_prefers_localappdata_over_userprofile",
            ])
            .env(MARKER, "1")
            .env("LOCALAPPDATA", r"C:\parvi-local")
            .env("USERPROFILE", r"C:\parvi-user")
            .env("HOME", r"C:\parvi-home")
            .env("XDG_DATA_HOME", r"C:\parvi-xdg")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
