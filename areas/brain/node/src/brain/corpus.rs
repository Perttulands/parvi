//! The corpus checkout: pages read off disk, with the links they make and the
//! pages that link to them.
//!
//! Every page is named by its corpus-relative path with `/` separators. A path
//! that is absolute, climbs out, passes through a dot-folder, or resolves
//! through a symlink to somewhere outside the corpus is refused. Links and
//! backlinks come from one walk of the corpus on each read: nothing is
//! indexed or cached, so the reader always sees the checkout as it is.

use parvi_brain_contract::{Collection, Link, Page, PageRef};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use axum::http::StatusCode;

use super::links::{Index, Reference, references};
use super::page;
use super::refusal::Refusal;

/// The most bytes read from one page: the Brain shows prose, not archives.
pub const PAGE_LIMIT: u64 = 1 << 20;

pub struct Corpus {
    root: PathBuf,
}

/// A page as the walk saw it.
struct Entry {
    page: PageRef,
    references: Vec<Reference>,
}

impl Corpus {
    /// The corpus at `root`, which must be a directory.
    pub fn open(root: &Path) -> Result<Corpus> {
        let root = root
            .canonicalize()
            .with_context(|| format!("brain corpus {}", root.display()))?;
        if !root.is_dir() {
            bail!("brain corpus {} is not a directory", root.display());
        }
        Ok(Corpus { root })
    }

    /// Reads the page at `path`, with its links and backlinks.
    pub fn page(&self, path: &str) -> Result<Page, Refusal> {
        let file = self.resolve(path)?;
        let text = read_page(&file).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Refusal::new(StatusCode::NOT_FOUND, "No such page"),
            _ => Refusal::new(StatusCode::FORBIDDEN, error.to_string()),
        })?;
        let parsed = page::parse(path, &text);
        let entries = self.walk();
        let index = Index::new(
            entries
                .iter()
                .map(|e| (e.page.path.as_str(), e.page.title.as_str())),
        );
        let named = |reference: &Reference| {
            index
                .resolve(&reference.to)
                .map(|at| entries[at].page.clone())
        };
        let mut links: Vec<Link> = Vec::new();
        for reference in references(path, parsed.body) {
            if links.iter().all(|l| l.target != reference.written) {
                links.push(Link {
                    page: named(&reference),
                    target: reference.written,
                });
            }
        }
        let mut backlinks: Vec<PageRef> = entries
            .iter()
            .filter(|e| e.page.path != path)
            .filter(|e| {
                e.references
                    .iter()
                    .any(|r| named(r).is_some_and(|p| p.path == path))
            })
            .map(|e| e.page.clone())
            .collect();
        backlinks.sort_by_key(|p| p.title.to_lowercase());
        Ok(Page {
            path: path.to_owned(),
            title: parsed.title,
            kind: parsed.kind,
            summary: parsed.summary,
            front: text[..text.len() - parsed.body.len()].to_owned(),
            body: parsed.body.to_owned(),
            links,
            backlinks,
            updated: String::new(),
            author: String::new(),
            history: Vec::new(),
            history_error: None,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The corpus's top-level folders with their page counts, by name.
    pub fn collections(&self) -> Vec<Collection> {
        let mut counts: Vec<Collection> = Vec::new();
        let Ok(listing) = fs::read_dir(&self.root) else {
            return counts;
        };
        for item in listing.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && item.file_type().is_ok_and(|k| k.is_dir()) {
                counts.push(Collection { name, pages: 0 });
            }
        }
        for (_, path) in self.files() {
            let top = path.split_once('/').map(|(top, _)| top);
            if let Some(c) = counts.iter_mut().find(|c| Some(c.name.as_str()) == top) {
                c.pages += 1;
            }
        }
        counts.sort_by(|a, b| a.name.cmp(&b.name));
        counts
    }

    /// The pages of one collection, by path.
    pub fn collection(&self, name: &str) -> Result<Vec<PageRef>, Refusal> {
        if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
            return Err(Refusal::new(StatusCode::FORBIDDEN, "No such collection"));
        }
        if !self.root.join(name).is_dir() {
            return Err(Refusal::new(StatusCode::NOT_FOUND, "No such collection"));
        }
        let prefix = format!("{name}/");
        Ok(self
            .files()
            .into_iter()
            .filter(|(_, path)| path.starts_with(&prefix))
            .filter_map(|(file, path)| reference(&file, path))
            .collect())
    }

    /// What the page at `path` is called, if it is there.
    pub fn page_ref(&self, path: &str) -> Option<PageRef> {
        reference(&self.resolve(path).ok()?, path.to_owned())
    }

    /// The file a page names, which must be there.
    pub fn existing(&self, path: &str) -> Result<PathBuf, Refusal> {
        let file = self.resolve(path)?;
        match file.is_file() {
            true => Ok(file),
            false => Err(Refusal::new(StatusCode::NOT_FOUND, "No such page")),
        }
    }

    /// The file a corpus-relative page path names, inside the corpus.
    fn resolve(&self, path: &str) -> Result<PathBuf, Refusal> {
        let outside = || Refusal::new(StatusCode::FORBIDDEN, "That path is outside the corpus");
        let relative = Path::new(path);
        let plain = relative.components().all(|c| match c {
            Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
            _ => false,
        });
        if path.is_empty() || path.contains('\\') || !plain {
            return Err(outside());
        }
        if page::strip_md(path) == path {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "A page is a .md file",
            ));
        }
        let file = self.root.join(relative);
        match file.canonicalize() {
            Ok(real) if real.starts_with(&self.root) => Ok(real),
            Ok(_) => Err(outside()),
            Err(_) => Err(Refusal::new(StatusCode::NOT_FOUND, "No such page")),
        }
    }

    /// Every page in the corpus, by path. Dot-folders and symlinks are left
    /// out, as Citadel leaves them out of its index.
    fn walk(&self) -> Vec<Entry> {
        self.files()
            .into_iter()
            .filter_map(|(file, path)| {
                let text = read_page(&file).ok()?;
                let parsed = page::parse(&path, &text);
                Some(Entry {
                    references: references(&path, parsed.body),
                    page: PageRef {
                        title: parsed.title,
                        kind: parsed.kind,
                        path,
                    },
                })
            })
            .collect()
    }

    /// Every page file and its corpus-relative path, by path.
    fn files(&self) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        let mut folders = vec![(self.root.clone(), String::new())];
        while let Some((folder, prefix)) = folders.pop() {
            let Ok(listing) = fs::read_dir(&folder) else {
                continue;
            };
            for item in listing.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                let Ok(kind) = item.file_type() else { continue };
                if name.starts_with('.') {
                    continue;
                }
                let path = format!("{prefix}{name}");
                if kind.is_dir() {
                    folders.push((item.path(), format!("{path}/")));
                } else if kind.is_file() && page::strip_md(&name) != name {
                    files.push((item.path(), path));
                }
            }
        }
        files.sort_by(|a, b| a.1.cmp(&b.1));
        files
    }
}

/// The page in `file`, at `path`, by its title and kind.
fn reference(file: &Path, path: String) -> Option<PageRef> {
    let text = read_page(file).ok()?;
    let parsed = page::parse(&path, &text);
    Some(PageRef {
        title: parsed.title,
        kind: parsed.kind,
        path,
    })
}

fn read_page(file: &Path) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(file)?
        .take(PAGE_LIMIT)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
