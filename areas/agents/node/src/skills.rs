//! The skills folders the harnesses read, and what each entry in them is: a
//! link into a checkout, a link that leads nowhere, or a plain copy. Skills
//! a checkout has that a folder lacks are reported as missing there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::Home;

/// Each skills folder: its id and where it is under the home folder.
pub const FOLDERS: &[(&str, &str)] = &[("claude", ".claude/skills"), ("agents", ".agents/skills")];

/// What a skill is in one folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// A link that resolves.
    Linked,
    /// A link whose target is gone.
    Broken,
    /// A folder of its own, not a link: nothing syncs it.
    Copy,
    /// Not in this folder, though a checkout another folder links into has it.
    Missing,
}

impl Link {
    pub fn word(self) -> &'static str {
        match self {
            Link::Linked => "linked",
            Link::Broken => "broken",
            Link::Copy => "copy",
            Link::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Where its files are: a link's target, or the copy.
    pub source: Option<PathBuf>,
    /// The checkout holding it, if any.
    pub checkout: Option<PathBuf>,
    /// Upstream commits not checked out that touch it.
    pub behind: u32,
    /// Source and checkout for each folder, in folder order.
    pub sources: Vec<Option<PathBuf>>,
    pub checkouts: Vec<Option<PathBuf>>,
    pub behind_by_folder: Vec<u32>,
    /// Its state in each of [`FOLDERS`], in that order.
    pub links: Vec<Link>,
}

pub fn folder(home: &Home, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(home.dir.clone(), |dir, part| dir.join(part))
}

/// Every skill in the folders, by name, and every skill the checkouts they
/// link into hold.
pub fn scan(home: &Home) -> Vec<Skill> {
    let mut skills: BTreeMap<String, Skill> = BTreeMap::new();
    for (index, (_, relative)) in FOLDERS.iter().enumerate() {
        let Ok(entries) = std::fs::read_dir(folder(home, relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Some((link, source)) = classify(&entry.path()) else {
                continue;
            };
            let skill = skills.entry(name.clone()).or_insert_with(|| blank(name));
            skill.links[index] = link;
            skill.sources[index] = source.clone();
            if link == Link::Linked {
                skill.checkouts[index] = source.as_deref().and_then(super::checkout::holding);
            }
            if skill.source.is_none()
                || link == Link::Linked && !skill.links[..index].contains(&Link::Linked)
            {
                skill.source = source;
            }
        }
    }
    // Each folder that linked skills come from, so its other skills show
    // as missing where they are not linked.
    let roots: Vec<PathBuf> = skills
        .values()
        .flat_map(|s| s.sources.iter().zip(&s.links))
        .filter(|(_, link)| **link == Link::Linked)
        .filter_map(|(source, _)| source.as_deref()?.parent().map(Path::to_owned))
        .collect();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().join("SKILL.md").is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            skills.entry(name.clone()).or_insert_with(|| Skill {
                source: Some(entry.path()),
                ..blank(name)
            });
        }
    }
    let mut skills: Vec<Skill> = skills.into_values().collect();
    for skill in &mut skills {
        if let Some(source) = &skill.source {
            skill.description = description(source);
            if skill.links.contains(&Link::Linked)
                || skill.links.iter().all(|&l| l == Link::Missing)
            {
                skill.checkout = super::checkout::holding(source);
            }
        }
    }
    skills
}

fn blank(name: String) -> Skill {
    Skill {
        name,
        description: String::new(),
        source: None,
        checkout: None,
        behind: 0,
        links: vec![Link::Missing; FOLDERS.len()],
        sources: vec![None; FOLDERS.len()],
        checkouts: vec![None; FOLDERS.len()],
        behind_by_folder: vec![0; FOLDERS.len()],
    }
}

/// A folder entry as a skill, with where its files are. Plain files are not
/// skills.
fn classify(path: &Path) -> Option<(Link, Option<PathBuf>)> {
    let meta = path.symlink_metadata().ok()?;
    if meta.file_type().is_symlink() {
        return Some(match path.canonicalize() {
            Ok(target) if target.is_dir() => (Link::Linked, Some(target)),
            _ => (Link::Broken, None),
        });
    }
    meta.is_dir().then(|| (Link::Copy, Some(path.to_owned())))
}

/// The `description:` in the skill's front matter, if any.
fn description(source: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(source.join("SKILL.md")) else {
        return String::new();
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    lines
        .take_while(|line| line.trim() != "---")
        .find_map(|line| line.strip_prefix("description:"))
        .map(|value| value.trim().trim_matches(['"', '\'']).to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn copy_does_not_hide_link_or_its_checkout() {
        let dir = std::env::temp_dir().join(format!("parvi-82-mixed-{}", std::process::id()));
        let kit = dir.join("kit");
        std::fs::create_dir_all(kit.join(".git")).unwrap();
        std::fs::create_dir_all(kit.join("skills/shared")).unwrap();
        std::fs::create_dir_all(kit.join("skills/other")).unwrap();
        std::fs::write(kit.join("skills/other/SKILL.md"), "---\n---\n").unwrap();
        std::fs::create_dir_all(dir.join(".claude/skills/shared")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(kit.join("skills/shared"), dir.join(".agents/skills/shared"))
            .unwrap();
        let skills = scan(&Home {
            dir: dir.clone(),
            path: vec![],
        });
        let shared = skills.iter().find(|s| s.name == "shared").unwrap();
        assert_eq!(shared.links, vec![Link::Copy, Link::Linked]);
        assert_eq!(shared.checkouts[1].as_ref(), Some(&kit));
        assert_eq!(super::super::report::checkout_paths(&skills), vec![kit]);
        assert!(skills.iter().any(|s| s.name == "other"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_the_description_from_front_matter() {
        let dir = std::env::temp_dir().join(format!("parvi-82-skill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: handoff\ndescription: \"Leave a handoff.\"\n---\n\ndescription: not this\n",
        )
        .unwrap();
        assert_eq!(description(&dir), "Leave a handoff.");
        std::fs::write(dir.join("SKILL.md"), "# no front matter\ndescription: no\n").unwrap();
        assert_eq!(description(&dir), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
