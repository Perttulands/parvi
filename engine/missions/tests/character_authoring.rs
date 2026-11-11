use parvi_missions::{Character, CharacterStore};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "parvi-97-character-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn character(name: &str) -> Character {
    Character {
        name: name.into(),
        harness: "claude-code".into(),
        effort: "medium".into(),
        ..Default::default()
    }
}

#[test]
fn create_replace_delete_and_validate_character() {
    let dir = Scratch::new();
    let store = CharacterStore::new(&dir.0);
    let mut saved = store.create(character("  My Builder  ")).unwrap();
    assert_eq!(saved.id, "my-builder");
    assert_eq!(saved.name, "My Builder");
    assert_eq!(
        store.create(saved.clone()).unwrap_err().code.as_str(),
        "character_exists"
    );
    let old_file = std::fs::read(dir.0.join("characters.json")).unwrap();
    saved.effort = "invalid".into();
    assert_eq!(
        store.put(saved.clone()).unwrap_err().code.as_str(),
        "invalid_character"
    );
    assert_eq!(
        std::fs::read(dir.0.join("characters.json")).unwrap(),
        old_file
    );
    saved.effort = "high".into();
    store.put(saved.clone()).unwrap();
    assert_eq!(store.list().unwrap(), vec![saved]);
    store.delete("my-builder").unwrap();
    assert!(store.list().unwrap().is_empty());
    assert_eq!(
        store.delete("my-builder").unwrap_err().code.as_str(),
        "not_found"
    );
}

#[cfg(unix)]
#[test]
fn writes_through_character_file_symlink() {
    use std::os::unix::fs::symlink;
    let dir = Scratch::new();
    let target = dir.0.join("target.json");
    std::fs::write(&target, "{\"schema\":1,\"characters\":[]}").unwrap();
    let link = dir.0.join("characters.json");
    symlink("target.json", &link).unwrap();
    CharacterStore::new(&dir.0)
        .create(character("Builder"))
        .unwrap();
    assert!(
        std::fs::symlink_metadata(link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(std::fs::read_to_string(target).unwrap().contains("Builder"));
}

#[test]
fn concurrent_writers_do_not_lose_characters() {
    let dir = Scratch::new();
    let store = CharacterStore::new(&dir.0);
    std::thread::scope(|scope| {
        for i in 0..16 {
            let store = store.clone();
            scope.spawn(move || {
                store.create(character(&format!("Agent {i}"))).unwrap();
            });
        }
    });
    assert_eq!(store.list().unwrap().len(), 16);
}

#[test]
fn usage_reports_known_slots_and_unreadable_missions() {
    let dir = Scratch::new();
    let missions = dir.0.join("missions");
    std::fs::create_dir_all(&missions).unwrap();
    std::fs::write(
        missions.join("work.mission.toml"),
        r#"schema = 1
id = "msn_work"
slug = "work"
title = "Work"
rev = 1
[[formation]]
id = "fmn_work"
type = "solo"
title = "Step"
[[formation.slot]]
id = "slot_work"
label = "Worker"
agentId = "builder"
controller = false
"#,
    )
    .unwrap();
    std::fs::write(missions.join("broken.mission.toml"), "broken = [").unwrap();
    let report = parvi_missions::Store::new(&dir.0)
        .roles()
        .usage("builder")
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.unreadable[0].name, "broken");
    assert_eq!(report.usage.len(), 1);
    assert_eq!(report.usage[0].slot_id, "slot_work");
}

#[test]
fn role_in_use_can_be_retired_and_restored_but_not_deleted() {
    let dir = Scratch::new();
    let missions = dir.0.join("missions");
    std::fs::create_dir_all(&missions).unwrap();
    std::fs::write(
        missions.join("work.mission.toml"),
        r#"schema = 1
id = "msn_work"
slug = "work"
title = "Work"
rev = 1
[[formation]]
id = "fmn_work"
type = "solo"
title = "Step"
[[formation.slot]]
id = "slot_work"
label = "Worker"
agentId = "critic"
controller = false
"#,
    )
    .unwrap();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let role = roles
        .create(parvi_missions::RoleCard {
            id: "critic".into(),
            display_name: "Critic".into(),
            kind: "reviewer".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        roles
            .delete("critic", &role.etag)
            .unwrap_err()
            .code
            .as_str(),
        "role_in_use"
    );
    let retired = roles.retire("critic", &role.etag).unwrap();
    assert_eq!(retired.card.status, "retired");
    let restored = roles.restore("critic", &retired.etag).unwrap();
    assert_eq!(restored.card.status, "active");
    std::fs::remove_file(missions.join("work.mission.toml")).unwrap();
    roles.delete("critic", &restored.etag).unwrap();
    assert!(roles.read("critic").is_err());
}

#[test]
fn builtin_role_restore_and_role_notes_preserve_usage_safety() {
    let dir = Scratch::new();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let builtin = roles.role_edit("builder").unwrap();
    let retired = roles.retire("builder", &builtin.etag).unwrap();
    assert_eq!(retired.card.status, "retired");
    let restored = roles.restore("builder", &retired.etag).unwrap();
    assert_eq!(restored.card.status, "active");
    assert!(!dir.0.join("roles/builder.toml").exists());
    let noted = roles
        .add_note(
            "builder",
            &restored.etag,
            " human:ui ",
            "Remember the tests",
        )
        .unwrap();
    assert_eq!(noted.card.notes[0].actor, "human:ui");
    assert_eq!(
        roles
            .add_note("builder", &restored.etag, "human:ui", "stale")
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
    let missions = dir.0.join("missions");
    std::fs::create_dir_all(&missions).unwrap();
    std::fs::write(missions.join("broken.mission.toml"), "schema = [").unwrap();
    assert_eq!(
        roles
            .delete("builder", &noted.etag)
            .unwrap_err()
            .code
            .as_str(),
        "role_usage_incomplete"
    );
}

#[test]
fn editing_role_capabilities_preserves_unknown_text_fields() {
    let dir = Scratch::new();
    let roles_dir = dir.0.join("roles");
    std::fs::create_dir_all(&roles_dir).unwrap();
    let path = roles_dir.join("susie.toml");
    std::fs::write(&path, "schema = 1\n\n[card]\nid = \"susie\"\ndisplay_name = \"Susie\"\nkind = \"specialist\"\ntags = [\"design\", \"taste:visual\"]\nreviewerNotes = \"prefers tight grids\"\n").unwrap();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let mut current = roles.role_edit("susie").unwrap();
    current.card.tags.push("tailwind".into());
    let mut updated = roles.edit("susie", &current.etag, current.card).unwrap();
    updated.card.tags.retain(|tag| tag != "tailwind");
    let updated = roles.edit("susie", &updated.etag, updated.card).unwrap();
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("reviewerNotes = \"prefers tight grids\"")
    );
    let unchanged = roles
        .edit("susie", &updated.etag, updated.card.clone())
        .unwrap();
    assert_eq!(unchanged.etag, updated.etag);
}

// Ported from agents_test.go: role cards contain text, preserve existing files,
// and require a fresh precondition before an edit.
#[test]
fn role_creation_and_edits_are_guarded_and_text_only() {
    let dir = Scratch::new();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let created = roles
        .create(parvi_missions::RoleCard {
            id: "researcher".into(),
            tags: vec!["research".into(), "go".into()],
            ..Default::default()
        })
        .unwrap();
    let path = dir.0.join("roles/researcher.toml");
    let raw = std::fs::read_to_string(&path).unwrap();
    for forbidden in ["harness", "model", "effort", "session"] {
        assert!(!raw.contains(forbidden));
    }
    assert_eq!(created.card.display_name, "researcher");
    assert_eq!(
        roles
            .create(created.card.clone())
            .unwrap_err()
            .code
            .as_str(),
        "already_exists"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    let mut next = created.card.clone();
    next.tags.push("react".into());
    assert_eq!(
        roles
            .edit("researcher", "", next.clone())
            .unwrap_err()
            .code
            .as_str(),
        "precondition_required"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    let updated = roles
        .edit("researcher", &created.etag, next.clone())
        .unwrap();
    assert_ne!(updated.etag, created.etag);
    next.tags.push("stale".into());
    assert_eq!(
        roles
            .edit("researcher", &created.etag, next)
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
    assert_eq!(roles.role_edit("researcher").unwrap().etag, updated.etag);
    assert_eq!(
        roles
            .create(parvi_missions::RoleCard {
                id: "bad/path".into(),
                ..Default::default()
            })
            .unwrap_err()
            .code
            .as_str(),
        "invalid_agent_card"
    );
}

// Ported from role_usage_test.go: every slot in every mission is reported.
#[test]
fn role_usage_reports_every_slot_in_every_mission() {
    let dir = Scratch::new();
    let store = parvi_missions::Store::new(&dir.0);
    for (slug, title) in [("one", "One"), ("two", "Two")] {
        let mut edit = store.create_mission(slug, title, "").unwrap();
        edit = store
            .patch(
                slug,
                parvi_mission_model::Precondition {
                    rev: edit.mission.rev,
                    etag: edit.etag.clone(),
                },
                parvi_mission_model::Patch::CreateFormation {
                    kind: "peer".into(),
                    title: "Pair".into(),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
        let formation = edit.mission.formations[0].clone();
        for slot in formation.slots {
            edit = store
                .patch(
                    slug,
                    parvi_mission_model::Precondition {
                        rev: edit.mission.rev,
                        etag: edit.etag.clone(),
                    },
                    parvi_mission_model::Patch::AssignSlot {
                        formation_id: formation.id.clone(),
                        slot_id: slot.id,
                        agent_id: "builder".into(),
                        harness: "claude-code".into(),
                        model: "".into(),
                        effort: "medium".into(),
                    },
                    "",
                )
                .unwrap();
        }
    }
    let report = store.roles().usage("builder").unwrap();
    assert!(report.complete);
    assert_eq!(report.usage.len(), 4);
    assert_eq!(
        report
            .usage
            .iter()
            .filter(|u| u.mission_slug == "one")
            .count(),
        2
    );
    assert_eq!(
        report
            .usage
            .iter()
            .filter(|u| u.mission_slug == "two")
            .count(),
        2
    );
    assert!(report.usage.iter().all(|u| u.formation_title == "Pair"));
    assert!(store.roles().usage("judge").unwrap().usage.is_empty());
}

#[test]
fn role_delete_requires_fresh_etag_and_restores_builtin_override() {
    let dir = Scratch::new();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let role = roles
        .create(parvi_missions::RoleCard {
            id: "writer".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        roles.delete("writer", "").unwrap_err().code.as_str(),
        "precondition_required"
    );
    assert_eq!(
        roles.delete("writer", "stale").unwrap_err().code.as_str(),
        "conflict"
    );
    roles.delete("writer", &role.etag).unwrap();
    assert_eq!(
        roles
            .delete("writer", &role.etag)
            .unwrap_err()
            .code
            .as_str(),
        "not_found"
    );
    let builtin = roles.role_edit("judge").unwrap();
    assert_eq!(
        roles
            .delete("judge", &builtin.etag)
            .unwrap_err()
            .code
            .as_str(),
        "builtin_role"
    );
    let mut override_card = builtin.card.clone();
    override_card.summary = "Judges plans only".into();
    let override_card = roles.edit("judge", &builtin.etag, override_card).unwrap();
    roles.delete("judge", &override_card.etag).unwrap();
    assert_eq!(roles.role_edit("judge").unwrap().card, builtin.card);
}

#[test]
fn restoring_builtin_fields_preserves_extra_card_text() {
    let dir = Scratch::new();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let original = roles.role_edit("builder").unwrap();
    let mut custom = original.card.clone();
    custom.summary = "custom summary".into();
    let custom = roles.edit("builder", &original.etag, custom).unwrap();
    let path = dir.0.join("roles/builder.toml");
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{raw}reviewerNotes = \"keep this\"\n")).unwrap();
    let current = roles.role_edit("builder").unwrap();
    assert_ne!(current.etag, custom.etag);
    let restored = roles
        .edit("builder", &current.etag, original.card.clone())
        .unwrap();
    assert_eq!(restored.card, original.card);
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("reviewerNotes = \"keep this\"")
    );
}

#[test]
fn role_api_returns_core_errors_with_codes_and_messages() {
    let dir = Scratch::new();
    let roles = parvi_missions::Store::new(&dir.0).roles();
    let role = roles
        .create(parvi_missions::RoleCard {
            id: "writer".into(),
            ..Default::default()
        })
        .unwrap();
    let error: parvi_missions::Error = roles.retire("writer", "stale").unwrap_err();
    assert_eq!(error.code, "conflict");
    assert!(!error.message.is_empty());
    assert_eq!(roles.role_edit("writer").unwrap().etag, role.etag);
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        serde_json::json!({"code":"conflict", "message":"role changed"})
    );
}
