//! The harnesses a seat can run, the efforts each accepts, and the Codex
//! models this host knows.

use std::path::{Path, PathBuf};

use serde::Deserialize;

struct Harness {
    id: &'static str,
    /// What the CLI's effort flag accepts: `claude --effort`, Codex's
    /// `model_reasoning_effort`, Pi's thinking level.
    efforts: &'static [&'static str],
}

const HARNESSES: &[Harness] = &[
    Harness {
        id: "claude-code",
        efforts: &["low", "medium", "high", "xhigh", "max"],
    },
    Harness {
        id: "openai-codex",
        efforts: &["low", "medium", "high", "xhigh", "max", "ultra"],
    },
    Harness {
        id: "pi",
        efforts: &["low", "medium", "high", "xhigh", "max"],
    },
];

/// The effort policy, one line, for messages that ask for an effort.
const EFFORT_POLICY: &str = "low for errands (verifier, scout); medium for making things (builder, debugger, operator); xhigh for architecture and review (reviewer, judge, architect, designer, planner, orchestrator); max for consequential reviews, chosen by hand";

/// What this host knows about harness models: the efforts each Codex model
/// accepts, from the Codex CLI's model cache.
#[derive(Debug, Default)]
pub struct Harnesses {
    codex_models: Vec<CodexModel>,
}

#[derive(Debug, Deserialize)]
struct CodexModel {
    #[serde(default)]
    slug: String,
    #[serde(default, rename = "supported_reasoning_levels")]
    levels: Vec<CodexLevel>,
}

#[derive(Debug, Deserialize)]
struct CodexLevel {
    #[serde(default)]
    effort: String,
}

#[derive(Deserialize)]
struct CodexCache {
    #[serde(default)]
    models: Vec<CodexModel>,
}

impl Harnesses {
    /// Reads `$CODEX_HOME/models_cache.json` (default `~/.codex`). A missing
    /// or malformed cache means no known Codex models.
    pub fn from_host() -> Harnesses {
        codex_home().map_or_else(Harnesses::default, |home| Harnesses::from_codex_home(&home))
    }

    /// Reads the model cache in a Codex home directory.
    pub fn from_codex_home(home: &Path) -> Harnesses {
        let cache = std::fs::read(home.join("models_cache.json")).ok();
        let cache: Option<CodexCache> = cache.and_then(|raw| serde_json::from_slice(&raw).ok());
        Harnesses {
            codex_models: cache.map(|cache| cache.models).unwrap_or_default(),
        }
    }

    /// The models the node knows for an authoring picker.
    pub fn models(&self, harness: &str) -> Vec<serde_json::Value> {
        match harness {
            "claude-code" => ["opus", "sonnet", "haiku", "fable"].iter().map(|id| serde_json::json!({"id":id})).collect(),
            "pi" => ["anthropic/claude-opus-5-5", "anthropic/claude-sonnet-5-5", "anthropic/claude-haiku-5-5", "anthropic/claude-fable-5-1"].iter().map(|id| serde_json::json!({"id":id})).collect(),
            "openai-codex" => self.codex_models.iter().filter(|m| !m.slug.is_empty()).map(|m| serde_json::json!({"id":m.slug,"efforts":m.levels.iter().map(|level| &level.effort).filter(|level| !level.is_empty()).collect::<Vec<_>>()})).collect(),
            _ => Vec::new(),
        }
    }

    /// Checks that a seat can start from a harness, model and effort; the
    /// error says why not. `subject` names what is checked, such as
    /// `slot "Worker" (w1)`.
    pub fn check(
        &self,
        subject: &str,
        harness: &str,
        model: &str,
        effort: &str,
    ) -> Result<(), String> {
        let ids = HARNESSES
            .iter()
            .map(|h| h.id)
            .collect::<Vec<_>>()
            .join(" or ");
        if harness.is_empty() {
            return Err(format!("{subject} needs a harness: {ids}"));
        }
        let Some(known) = HARNESSES.iter().find(|h| h.id == harness) else {
            return Err(format!(
                "{subject} harness {harness:?} cannot start seats; use {ids}"
            ));
        };
        if model.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!(
                "{subject} model {model:?} must be one model name without spaces"
            ));
        }
        if effort.is_empty() {
            return Err(format!(
                "{subject} needs an effort; the policy is {EFFORT_POLICY}"
            ));
        }
        // A model whose levels the host knows narrows the harness's efforts.
        let (efforts, accepts) = match self.model_efforts(harness, model) {
            Some(efforts) => (efforts, model),
            None => (
                known.efforts.iter().map(|e| e.to_string()).collect(),
                harness,
            ),
        };
        if !efforts.iter().any(|candidate| candidate == effort) {
            return Err(format!(
                "{subject} effort {effort:?} is not one {accepts} accepts; use {}",
                efforts.join(", ")
            ));
        }
        Ok(())
    }

    fn model_efforts(&self, harness: &str, model: &str) -> Option<Vec<String>> {
        if harness != "openai-codex" || model.is_empty() {
            return None;
        }
        let known = self.codex_models.iter().find(|m| m.slug == model)?;
        let efforts: Vec<_> = (known.levels.iter())
            .filter(|level| !level.effort.is_empty())
            .map(|level| level.effort.clone())
            .collect();
        (!efforts.is_empty()).then_some(efforts)
    }
}

fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => Some(crate::store::home_dir()?.join(".codex")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_codex_model_narrows_the_efforts() {
        let home = std::env::temp_dir().join(format!("parvi-64-codex-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("models_cache.json"),
            r#"{"models":[{"slug":"gpt-small","priority":1,"supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"}]}]}"#,
        )
        .unwrap();
        let harnesses = Harnesses::from_codex_home(&home);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(
            harnesses.check("slot", "openai-codex", "gpt-small", "medium"),
            Ok(())
        );
        assert_eq!(
            harnesses.check("slot", "openai-codex", "gpt-small", "ultra"),
            Err(r#"slot effort "ultra" is not one gpt-small accepts; use low, medium"#.to_owned())
        );
        assert_eq!(
            harnesses.check("slot", "openai-codex", "gpt-other", "ultra"),
            Ok(())
        );
        assert_eq!(
            harnesses
                .check("slot", "claude-code", "opus", "ultra")
                .unwrap_err(),
            r#"slot effort "ultra" is not one claude-code accepts; use low, medium, high, xhigh, max"#
        );
    }

    #[test]
    fn a_seat_needs_a_known_harness_one_model_and_an_effort() {
        let harnesses = Harnesses::default();
        assert!(
            harnesses
                .check("s", "", "", "low")
                .unwrap_err()
                .contains("needs a harness")
        );
        assert!(
            harnesses
                .check("s", "hermes", "", "low")
                .unwrap_err()
                .contains("cannot start seats")
        );
        assert!(
            harnesses
                .check("s", "pi", "a b", "low")
                .unwrap_err()
                .contains("without spaces")
        );
        assert!(
            harnesses
                .check("s", "pi", "", "")
                .unwrap_err()
                .contains("needs an effort")
        );
    }
}

mod adapter;
mod conversation;
mod launch;
mod tokens;
mod transcript;

pub use adapter::{
    Claude, Codex, HarnessAdapter, Keys, Launch, Pi, Pointer, Shell, SlotBinding, TokenUsage,
    TranscriptRoots, Turn, TurnState,
};

pub use conversation::codex_threads;
