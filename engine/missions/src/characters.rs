//! Characters: saved staffings. A character is a stamp: stamping a slot
//! writes its role, harness, model and effort into the slot and keeps no
//! link, so editing or deleting a character changes no mission. They live in
//! `characters.json` beside the role cards.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::links;

pub const CHARACTERS_SCHEMA: i64 = 1;
pub const CHARACTERS_FILE: &str = "characters.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Character {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// A small PNG, JPEG or WebP image as a data URL; blank shows initials.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub picture: String,
    /// The role; blank is a vanilla agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    pub harness: String,
    /// Blank is the harness default model.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub effort: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CharactersFile {
    schema: i64,
    #[serde(default)]
    characters: Vec<Character>,
}

/// Reads the characters in a roles directory; none when it has no file.
pub fn read_characters(roles_dir: &Path) -> Result<Vec<Character>, Error> {
    let text = match links::read_definition(&roles_dir.join(CHARACTERS_FILE)) {
        Err(error) if error.code == "not_found" => return Ok(Vec::new()),
        other => other?,
    };
    let file: CharactersFile = serde_json::from_str(&text).map_err(|err| {
        Error::new(
            "invalid_definition_source",
            format!("{CHARACTERS_FILE}: {err}"),
        )
    })?;
    if file.schema != CHARACTERS_SCHEMA {
        return Err(Error::new(
            "unsupported_schema",
            format!(
                "{CHARACTERS_FILE} has schema {}, want {CHARACTERS_SCHEMA}",
                file.schema
            ),
        ));
    }
    Ok(file.characters)
}

/// Writes saved staffings independently of missions.
#[derive(Clone, Debug)]
pub struct CharacterStore {
    dir: PathBuf,
}

impl CharacterStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn list(&self) -> Result<Vec<Character>, crate::Error> {
        let locks = crate::write::locks(&self.dir, "characters");
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        read_characters(&self.dir)
    }

    pub fn create(&self, character: Character) -> Result<Character, crate::Error> {
        self.save(character, true)
    }

    pub fn put(&self, character: Character) -> Result<Character, crate::Error> {
        self.save(character, false)
    }

    pub fn delete(&self, id: &str) -> Result<(), crate::Error> {
        let locks = crate::write::locks(&self.dir, "characters");
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let mut characters = read_characters(&self.dir)?;
        let index = characters
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| crate::Error::new("not_found", format!("character {id:?}")))?;
        characters.remove(index);
        self.write(characters)
    }

    fn save(&self, mut character: Character, create: bool) -> Result<Character, crate::Error> {
        character.name = character.name.trim().to_owned();
        character.description = character.description.trim().to_owned();
        character.id = character.id.trim().to_owned();
        if character.id.is_empty() {
            character.id = character_id(&character.name);
        }
        let invalid = |message: String| crate::Error::new("invalid_character", message);
        if character.name.is_empty()
            || !valid_id(&character.id)
            || (!character.agent_id.is_empty() && !valid_id(&character.agent_id))
        {
            return Err(invalid(
                "character needs a name and valid id and role".into(),
            ));
        }
        crate::Harnesses::from_host()
            .check(
                "character",
                &character.harness,
                &character.model,
                &character.effort,
            )
            .map_err(invalid)?;
        if !character.picture.is_empty()
            && (character.picture.len() > 96 * 1024
                || ![
                    "data:image/png;base64,",
                    "data:image/jpeg;base64,",
                    "data:image/webp;base64,",
                ]
                .iter()
                .any(|prefix| character.picture.starts_with(prefix)))
        {
            return Err(invalid(
                "picture must be a PNG, JPEG or WebP data URL of at most 96 KiB".into(),
            ));
        }
        let locks = crate::write::locks(&self.dir, "characters");
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let mut characters = read_characters(&self.dir)?;
        match (characters.iter().position(|c| c.id == character.id), create) {
            (Some(_), true) => {
                return Err(crate::Error::new(
                    "character_exists",
                    format!("character {:?} exists", character.id),
                ));
            }
            (None, false) => {
                return Err(crate::Error::new(
                    "not_found",
                    format!("character {:?}", character.id),
                ));
            }
            (Some(index), false) => characters[index] = character.clone(),
            (None, true) => characters.push(character.clone()),
        }
        self.write(characters)?;
        Ok(character)
    }

    fn write(&self, mut characters: Vec<Character>) -> Result<(), crate::Error> {
        characters.sort_by_cached_key(|c| (c.name.to_lowercase(), c.id.clone()));
        let mut bytes = serde_json::to_vec_pretty(&CharactersFile {
            schema: CHARACTERS_SCHEMA,
            characters,
        })
        .map_err(|e| crate::Error::new("invalid_character", e.to_string()))?;
        bytes.push(b'\n');
        crate::write::atomic(&self.dir.join(CHARACTERS_FILE), &bytes).map_err(crate::error::io)?;
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn character_id(name: &str) -> String {
    let mut id = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !id.is_empty() {
                id.push('-');
            }
            id.push(c);
            dash = false;
        } else {
            dash = true;
        }
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_read_from_the_roles_directory() {
        let dir = std::env::temp_dir().join(format!("parvi-64-characters-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_characters(&dir).unwrap(), Vec::new());
        std::fs::write(
            dir.join(CHARACTERS_FILE),
            r#"{"schema":1,"characters":[{"id":"astra","name":"Astra","agentId":"reviewer","harness":"openai-codex","model":"gpt-6-astra","effort":"max"}]}"#,
        )
        .unwrap();
        let characters = read_characters(&dir).unwrap();
        assert_eq!(
            (characters[0].name.as_str(), characters[0].effort.as_str()),
            ("Astra", "max")
        );
        std::fs::write(dir.join(CHARACTERS_FILE), r#"{"schema":2,"characters":[]}"#).unwrap();
        assert_eq!(
            read_characters(&dir).unwrap_err().code,
            "unsupported_schema"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
