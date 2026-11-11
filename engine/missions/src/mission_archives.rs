//! Mission definition, layout and note archives. Removing an archived
//! symlink unlinks it and never removes its target.
use crate::{Error, Store};
use parvi_mission_model::{Mission, Notes};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The archived components that remain for one deletion or old note edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissionArchive {
    pub slug: String,
    pub archive_id: String,
    pub mission: bool,
    pub layout: bool,
    pub notes: bool,
    /// Notes retained by an older delete, associated only by missionId.
    pub retained_notes: bool,
}

fn check_archive_id(id: &str) -> Result<(), Error> {
    let suffix = id.strip_prefix("archive_").unwrap_or("");
    if suffix.is_empty()
        || suffix.len() > 64
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::new(
            "invalid_name",
            format!("mission archive id {id:?}"),
        ));
    }
    Ok(())
}
fn archived_mission_id(store: &Store, slug: &str, archive_id: &str) -> Option<String> {
    let path = store
        .root()
        .join("missions")
        .join(format!("{slug}.mission.toml.deleted-{archive_id}"));
    Mission::parse(&crate::links::read_definition(&path).ok()?)
        .ok()
        .map(|mission| mission.id)
}
/// An old delete left notes at the live path. Claim them only when exactly one
/// readable archived definition has the same mission ID and no live mission
/// owns that ID. An unreadable definition makes ownership ambiguous.
fn legacy_owner(store: &Store, archive: &MissionArchive, all: &[MissionArchive]) -> bool {
    if !archive.mission {
        return false;
    }
    let path = store
        .root()
        .join("notes")
        .join(format!("{}.notes.toml", archive.slug));
    let Some(notes) = crate::links::read_definition(&path)
        .ok()
        .and_then(|raw| Notes::parse(&raw).ok())
    else {
        return false;
    };
    let live = store
        .root()
        .join("missions")
        .join(format!("{}.mission.toml", archive.slug));
    match crate::links::read_definition(&live) {
        Ok(raw) => match Mission::parse(&raw) {
            Ok(mission) if mission.id != notes.mission_id => (),
            _ => return false,
        },
        Err(error) if error.code == "not_found" => (),
        Err(_) => return false,
    }
    let mut owners = Vec::new();
    for candidate in all
        .iter()
        .filter(|other| other.slug == archive.slug && other.mission)
    {
        let Some(id) = archived_mission_id(store, &candidate.slug, &candidate.archive_id) else {
            return false;
        };
        if id == notes.mission_id {
            owners.push(candidate.archive_id.as_str());
        }
    }
    owners.as_slice() == [archive.archive_id.as_str()]
}

/// Archive as one operation where possible; if a rename fails, restore every
/// earlier move. Any component that could not be rolled back remains listable.
pub(crate) fn move_archives(moves: Vec<(PathBuf, PathBuf)>) -> Result<(), Error> {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (source, destination) in moves {
        if let Err(error) = std::fs::rename(&source, &destination) {
            let mut failures = Vec::new();
            for (original, archived) in moved.into_iter().rev() {
                if let Err(rollback) = std::fs::rename(&archived, &original) {
                    failures.push(format!("{}: {rollback}", original.display()));
                }
            }
            if !failures.is_empty() {
                return Err(Error::new(
                    "io",
                    format!(
                        "archive failed: {error}; rollback failed for {}",
                        failures.join(", ")
                    ),
                ));
            }
            return Err(crate::error::io(error));
        }
        moved.push((source, destination));
    }
    Ok(())
}

impl Store {
    /// List the independently archived files, including orphan components.
    /// Active legacy notes are associated only when ownership is provable.
    pub fn mission_archives(&self) -> Result<Vec<MissionArchive>, Error> {
        let mut found = BTreeMap::<(String, String), MissionArchive>::new();
        for (folder, marker) in [
            ("missions", ".mission.toml.deleted-"),
            ("layout", ".layout.toml.deleted-"),
            ("notes", ".notes.toml.deleted-"),
        ] {
            let dir = self.root().join(folder);
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(crate::error::io(err)),
            };
            for entry in entries {
                let entry = entry.map_err(crate::error::io)?;
                let name = entry.file_name();
                let Some((slug, id)) = name.to_str().and_then(|name| name.rsplit_once(marker))
                else {
                    continue;
                };
                if crate::store::check_slug(slug).is_err() || check_archive_id(id).is_err() {
                    continue;
                }
                let archive =
                    found
                        .entry((slug.into(), id.into()))
                        .or_insert_with(|| MissionArchive {
                            slug: slug.into(),
                            archive_id: id.into(),
                            mission: false,
                            layout: false,
                            notes: false,
                            retained_notes: false,
                        });
                match folder {
                    "missions" => archive.mission = true,
                    "layout" => archive.layout = true,
                    "notes" => archive.notes = true,
                    _ => unreachable!(),
                }
            }
        }
        let mut archives: Vec<_> = found.into_values().collect();
        let all = archives.clone();
        for archive in &mut archives {
            archive.retained_notes = legacy_owner(self, archive, &all);
        }
        Ok(archives)
    }

    /// Remove only the named archived files and any proven legacy notes. A
    /// partial failure remains discoverable and retryable. Live definitions,
    /// unrelated notes and runs are never removed.
    pub fn remove_mission_archive(
        &self,
        slug: &str,
        archive_id: &str,
    ) -> Result<MissionArchive, Error> {
        crate::store::check_slug(slug)?;
        check_archive_id(archive_id)?;
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let archive = self
            .mission_archives()?
            .into_iter()
            .find(|archive| archive.slug == slug && archive.archive_id == archive_id)
            .ok_or_else(|| {
                Error::new(
                    "not_found",
                    format!("mission archive {slug:?} {archive_id:?}"),
                )
            })?;
        let remove = |path: &Path| -> Result<bool, Error> {
            match std::fs::remove_file(path) {
                Ok(()) => Ok(true),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(err) => Err(crate::error::io(err)),
            }
        };
        // Remove legacy notes while the definition that proves ownership still
        // exists. If a later removal fails, the remaining parts stay listed.
        let retained_notes = if archive.retained_notes {
            remove(&self.root().join("notes").join(format!("{slug}.notes.toml")))?
        } else {
            false
        };
        let notes = remove(
            &self
                .root()
                .join("notes")
                .join(format!("{slug}.notes.toml.deleted-{archive_id}")),
        )?;
        let layout = remove(
            &self
                .root()
                .join("layout")
                .join(format!("{slug}.layout.toml.deleted-{archive_id}")),
        )?;
        let mission = remove(
            &self
                .root()
                .join("missions")
                .join(format!("{slug}.mission.toml.deleted-{archive_id}")),
        )?;
        Ok(MissionArchive {
            slug: slug.into(),
            archive_id: archive_id.into(),
            mission,
            layout,
            notes,
            retained_notes,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_later_archive_move_restores_the_earlier_notes() {
        let dir =
            std::env::temp_dir().join(format!("parvi-archive-rollback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let notes = dir.join("live.notes.toml");
        let archived = dir.join("notes.deleted");
        let missing = dir.join("missing-layout");
        std::fs::write(&notes, "notes").unwrap();
        assert!(
            super::move_archives(vec![
                (notes.clone(), archived.clone()),
                (missing, dir.join("layout.deleted"))
            ])
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "notes");
        assert!(!archived.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
