//! Link: put a link to each skill a checkout has into every skills folder
//! that lacks it, so every harness reads it from the checkout. Links that
//! lead nowhere, and folders of their own, are left as they are.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::skills::{self, FOLDERS, Link};
use super::{Home, sync};

/// One link made: the skill and the folder it now stands in, as the
/// operator knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub skill: String,
    pub folder: String,
}

/// Links the skills of the checkout the operator knows as `shown` into the
/// folders missing them. Returns what it linked, nothing when all were.
pub fn link(home: &Home, shown: &str) -> Result<Vec<Linked>> {
    let checkout = sync::checkout(home, shown)?;
    let mut made = vec![];
    for skill in skills::scan(home) {
        let Some(source) = skill.source.as_deref().filter(|s| s.starts_with(&checkout)) else {
            continue;
        };
        for (index, (_, relative)) in FOLDERS.iter().enumerate() {
            if skill.links[index] != Link::Missing {
                continue;
            }
            let folder = skills::folder(home, relative);
            std::fs::create_dir_all(&folder)
                .with_context(|| format!("making {}", home.show(&folder)))?;
            let at = folder.join(&skill.name);
            symlink(source, &at).with_context(|| format!("linking {}", home.show(&at)))?;
            made.push(Linked {
                skill: skill.name.clone(),
                folder: home.show(&folder),
            });
        }
    }
    Ok(made)
}

#[cfg(unix)]
fn symlink(source: &Path, at: &PathBuf) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, at)
}

#[cfg(windows)]
fn symlink(source: &Path, at: &PathBuf) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, at)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn links_the_missing_skills_of_one_checkout_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("parvi-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let kit = dir.join("kit");
        let other = dir.join("other");
        for (repo, skill) in [(&kit, "shared"), (&kit, "fresh"), (&other, "theirs")] {
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(repo.join("skills").join(skill)).unwrap();
            std::fs::write(
                repo.join("skills").join(skill).join("SKILL.md"),
                "---\n---\n",
            )
            .unwrap();
        }
        std::fs::create_dir_all(dir.join(".claude/skills")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills")).unwrap();
        // shared is linked in one folder only; fresh in none; theirs belongs
        // to another checkout, missing from .agents.
        std::os::unix::fs::symlink(kit.join("skills/shared"), dir.join(".claude/skills/shared"))
            .unwrap();
        std::os::unix::fs::symlink(
            other.join("skills/theirs"),
            dir.join(".claude/skills/theirs"),
        )
        .unwrap();
        // A link that leads nowhere stays as it is.
        std::os::unix::fs::symlink(dir.join("gone"), dir.join(".agents/skills/gone")).unwrap();
        let home = Home {
            dir: dir.clone(),
            path: vec![],
        };
        let shown = home.show(&kit);
        let mut made = link(&home, &shown).unwrap();
        made.sort_by(|a, b| (&a.skill, &a.folder).cmp(&(&b.skill, &b.folder)));
        let pairs: Vec<(&str, &str)> = made
            .iter()
            .map(|l| (l.skill.as_str(), l.folder.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("fresh", "~/.agents/skills"),
                ("fresh", "~/.claude/skills"),
                ("shared", "~/.agents/skills"),
            ]
        );
        assert_eq!(
            std::fs::read_link(dir.join(".agents/skills/shared")).unwrap(),
            kit.join("skills/shared").canonicalize().unwrap()
        );
        assert!(!dir.join(".agents/skills/theirs").exists());
        assert!(dir.join(".agents/skills/gone").symlink_metadata().is_ok());
        // Done once, there is nothing left to link.
        assert!(link(&home, &shown).unwrap().is_empty());
        assert!(link(&home, "~/not-a-checkout").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
