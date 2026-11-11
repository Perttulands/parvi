//! Brain: the context corpus. Search goes through Context Citadel; pages are
//! read from the corpus checkout, with the links they make, the pages that
//! link to them and their history in git. The operator's own edits are
//! written back, each a commit. Either half may be left unconfigured, and
//! then says so.
//!
//! Routes, each answering JSON or the shared coded error:
//! - `GET /api/brain/search?q=` — Citadel's ranked results, best first.
//! - `GET /api/brain/page?path=` — one page by corpus-relative path.
//! - `PUT /api/brain/page` — `{ path, content, summary, base? }`: the page's new
//!   text, committed alone; answers the commit.
//! - `GET /api/brain/collections` — the top-level folders and their counts.
//! - `GET /api/brain/collection?name=` — one collection's pages, each with
//!   when it last changed.
//! - `GET /api/brain/changes?limit=` — the newest commits and the pages
//!   each touched.

mod citadel;
mod corpus;
mod history;
mod links;
mod page;
mod refusal;
mod routes;

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use axum::http::StatusCode;

pub use parvi_brain_contract::{Change, Collection, Commit, Found, Listed, Page, PageRef};

pub use refusal::{Refusal, code as refusal_code};
pub use routes::routes;

/// How many commits the changes read when not asked, and at most.
const CHANGES_DEFAULT: usize = 30;
const CHANGES_LIMIT: usize = 200;

/// Where this node's corpus and search are.
pub struct Brain {
    corpus: Option<Arc<corpus::Corpus>>,
    citadel: Option<citadel::Citadel>,
    author: history::Author,
    /// One save at a time: each checks the page is clean before it writes.
    saving: tokio::sync::Mutex<()>,
}

/// What git could read, and why not the rest: a corpus git refused and one
/// with nothing in it differ only by `error`.
#[derive(Debug)]
pub struct Read<T> {
    pub found: T,
    pub error: Option<String>,
}

impl Brain {
    /// The Brain over the corpus checkout at `corpus`, searched through the
    /// Citadel at `citadel` (`http://host:port`). A root that is not a
    /// directory or a URL Parvi cannot reach is an error now rather than at
    /// the first search.
    pub fn new(corpus: Option<&Path>, citadel: Option<&str>) -> Result<Brain> {
        Ok(Brain {
            corpus: corpus.map(corpus::Corpus::open).transpose()?.map(Arc::new),
            citadel: citadel.map(citadel::Citadel::new).transpose()?,
            author: history::Author::default(),
            saving: tokio::sync::Mutex::new(()),
        })
    }

    /// Signs edits as `author`, `Name <email>`; without one, git's own
    /// identity for the corpus signs them.
    pub fn with_author(mut self, author: Option<&str>) -> Result<Brain> {
        self.author = history::Author::parse(author)?;
        Ok(self)
    }

    /// Searches through Citadel, as the Brain tab's search does.
    pub async fn search(&self, query: &str) -> Result<Vec<Found>, Refusal> {
        let Some(citadel) = &self.citadel else {
            return Err(unconfigured(
                "No Citadel is configured for search ([brain].citadel)",
            ));
        };
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        citadel.search(query.trim()).await
    }

    /// One page of the corpus, as the Brain tab's reader shows it, with its
    /// history.
    pub async fn page(&self, path: &str) -> Result<Page, Refusal> {
        let corpus = self.corpus()?;
        let owned = path.to_owned();
        let mut page = blocking(corpus.clone(), move |c| c.page(&owned)).await?;
        let (history, error) = history::page(corpus.root(), &page.path).await;
        if let Some(last) = history.first() {
            page.updated = last.time.clone();
            page.author = last.author.clone();
        }
        page.history = history;
        page.history_error = error;
        Ok(page)
    }

    /// Writes the page at `path` and commits it, signed as the operator, with
    /// `summary` as its message (`Edit <path>` when empty).
    pub async fn save(
        &self,
        path: &str,
        text: &str,
        summary: &str,
        base: Option<&str>,
    ) -> Result<Commit, Refusal> {
        let corpus = self.corpus()?;
        if text.len() as u64 > corpus::PAGE_LIMIT {
            let why = "A page is at most 1 MiB, which the reader reads";
            return Err(Refusal::new(StatusCode::PAYLOAD_TOO_LARGE, why));
        }
        let owned = path.to_owned();
        let file = blocking(corpus.clone(), move |c| c.existing(&owned)).await?;
        let message = match summary.trim() {
            "" => format!("Edit {path}"),
            given => given.to_owned(),
        };
        let _one = self.saving.lock().await;
        if let Some(base) = base {
            let (history, error) = history::page(corpus.root(), path).await;
            if let Some(error) = error {
                return Err(Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error));
            }
            let current = history.first().map_or("", |commit| commit.hash.as_str());
            if current != base {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    format!("{path} changed since the base revision; read it again before saving"),
                ));
            }
        }
        history::save(corpus.root(), &file, path, text, &message, &self.author).await
    }

    /// The corpus's collections with their page counts.
    pub async fn collections(&self) -> Result<Vec<Collection>, Refusal> {
        blocking(self.corpus()?, |c| Ok(c.collections())).await
    }

    /// One collection's pages, each with when it last changed and by whom.
    pub async fn collection(&self, name: &str) -> Result<Read<Vec<Listed>>, Refusal> {
        let corpus = self.corpus()?;
        let owned = name.to_owned();
        let pages = blocking(corpus.clone(), move |c| c.collection(&owned)).await?;
        let (last, error) = history::last_changes(corpus.root(), name).await;
        let found = pages
            .into_iter()
            .map(|page| {
                let commit = last.get(&page.path).cloned().unwrap_or_default();
                Listed {
                    page,
                    updated: commit.time,
                    author: commit.author,
                }
            })
            .collect();
        Ok(Read { found, error })
    }

    /// The newest `limit` commits, each with the pages it touched that are
    /// still in the corpus, by title.
    pub async fn changes(&self, limit: Option<usize>) -> Result<Read<Vec<Change>>, Refusal> {
        let corpus = self.corpus()?;
        let limit = limit.unwrap_or(CHANGES_DEFAULT).min(CHANGES_LIMIT);
        let (log, error) = history::changes(corpus.root(), limit).await;
        let found = blocking(corpus, move |c| {
            Ok(log
                .into_iter()
                .map(|change| Change {
                    pages: change.files.iter().filter_map(|f| c.page_ref(f)).collect(),
                    commit: change.commit,
                })
                .collect())
        })
        .await?;
        Ok(Read { found, error })
    }

    fn corpus(&self) -> Result<Arc<corpus::Corpus>, Refusal> {
        self.corpus
            .clone()
            .ok_or_else(|| unconfigured("No corpus is configured ([brain].corpus)"))
    }
}

/// Runs a read of the corpus off the async threads.
async fn blocking<T: Send + 'static>(
    corpus: Arc<corpus::Corpus>,
    read: impl FnOnce(&corpus::Corpus) -> Result<T, Refusal> + Send + 'static,
) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(move || read(&corpus))
        .await
        .unwrap_or_else(|error| {
            Err(Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            ))
        })
}

fn unconfigured(message: &str) -> Refusal {
    Refusal::new(StatusCode::SERVICE_UNAVAILABLE, message)
}
