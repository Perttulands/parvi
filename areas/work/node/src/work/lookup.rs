//! The seam's on-demand side: resolving a written reference, reading one
//! item with its family, and naming an item to an agent. Each reads only
//! when asked (`docs/work.md`, The seam).

use anyhow::{Context, Result, bail};
use parvi_work_ref::{WorkRef, Written};
use tokio::task::JoinSet;

use super::config::Project;
use super::item::Item;
use super::poller::Work;

impl Work {
    /// The canonical reference for `text` in any written form. A bare Bead
    /// id resolves only when exactly one configured store has its prefix,
    /// and not while a store's prefix cannot be read.
    pub async fn resolve(&self, text: &str) -> Result<WorkRef> {
        let id = match parvi_work_ref::parse(text)? {
            Written::Ref(r) => return Ok(r),
            Written::Bead(id) => id,
        };
        let mut holders = Vec::new();
        for (folder, prefix) in self.prefixes().await {
            let prefix = prefix.with_context(|| format!("placing {id}"))?;
            if id.starts_with(&format!("{prefix}-")) {
                holders.push(folder);
            }
        }
        match holders.as_slice() {
            [folder] => Ok(WorkRef::Bead {
                node: self.0.node.clone(),
                folder: folder.clone(),
                id,
            }),
            [] => bail!(
                "no Beads store in this node's [work] projects has the prefix of {id}; add a beads project"
            ),
            many => bail!(
                "{id} could be in {}; name its store as bead:{}:<folder>#{id}",
                many.join(" or "),
                self.0.node
            ),
        }
    }

    /// One item with its family. A Bead comes from its store as last read
    /// if unchanged since, else from a read now; an issue is read now.
    pub async fn item(&self, reference: &WorkRef) -> Result<Item> {
        match reference {
            WorkRef::Issue {
                owner,
                repo,
                number,
            } => self.0.gh.item(owner, repo, *number).await,
            WorkRef::Bead { node, folder, id } => {
                if *node != self.0.node {
                    bail!("{reference} is on node {node}; read it there");
                }
                if !self.stores().any(|f| f == folder) {
                    bail!("{folder} is not a Beads project in this node's [work] projects");
                }
                let (items, _) = self.0.store(folder).await?;
                items
                    .iter()
                    .find(|item| item.id == *id)
                    .cloned()
                    .ok_or_else(|| {
                        parvi_core::Error::new("not_found", format!("no Bead {id} in {folder}"))
                            .into()
                    })
            }
        }
    }

    /// The line that names `item` to an agent ([`Item::reference_line`]),
    /// with its store's folder when another configured store shares the
    /// Bead's prefix.
    pub async fn reference_line(&self, item: &Item) -> String {
        let Ok(WorkRef::Bead { folder, id, .. }) = parvi_work_ref::parse_resolved(&item.reference)
        else {
            return item.reference_line(None);
        };
        let sharing = self
            .prefixes()
            .await
            .into_iter()
            .filter(|(_, prefix)| {
                prefix
                    .as_ref()
                    .is_ok_and(|p| id.starts_with(&format!("{p}-")))
            })
            .count();
        item.reference_line((sharing > 1).then_some(folder.as_str()))
    }

    fn stores(&self) -> impl Iterator<Item = &String> {
        self.0.config.projects.iter().filter_map(|p| match p {
            Project::Beads(store) => Some(&store.folder),
            Project::GitHub(_) => None,
        })
    }

    /// Every configured store's prefix, asking bd, all at once, for those
    /// not yet known.
    async fn prefixes(&self) -> Vec<(String, Result<String>)> {
        let mut asks = JoinSet::new();
        let mut known = Vec::new();
        for folder in self.stores() {
            match self.0.prefixes.lock().unwrap().get(folder) {
                Some(prefix) => known.push((folder.clone(), Ok(prefix.clone()))),
                None => {
                    let (bd, folder) = (self.0.bd.clone(), folder.clone());
                    asks.spawn(async move {
                        let prefix = bd.prefix(&folder).await;
                        (folder, prefix)
                    });
                }
            }
        }
        while let Some(asked) = asks.join_next().await {
            let (folder, prefix) = asked.expect("a prefix ask does not panic");
            if let Ok(prefix) = &prefix {
                self.0
                    .prefixes
                    .lock()
                    .unwrap()
                    .insert(folder.clone(), prefix.clone());
            }
            known.push((folder, prefix));
        }
        known
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::config::{Repo, Store};
    use super::super::poller::tests::{Fake, fixture};
    use super::*;

    /// A bd that answers `config get` with the prefix named in the store
    /// folder's `prefix` file, and `list` with the fixture.
    fn bd() -> String {
        format!(
            "case \"$*\" in *config*) echo \"{{\\\"value\\\":\\\"$(cat prefix)\\\"}}\";; *) {};; esac",
            fixture("beads.json")
        )
    }

    fn stores(fake: &Fake, prefixes: &[&str]) -> Vec<Project> {
        prefixes
            .iter()
            .enumerate()
            .map(|(i, prefix)| {
                let folder = fake.dir.join(format!("p{i}"));
                std::fs::create_dir_all(folder.join(".beads")).unwrap();
                std::fs::write(folder.join("prefix"), prefix).unwrap();
                Project::Beads(Store {
                    folder: folder.display().to_string(),
                })
            })
            .collect()
    }

    #[tokio::test]
    async fn resolves_a_bare_id_only_to_the_one_store_with_its_prefix() {
        let fake = Fake::new("resolve", "exit 1", &bd());
        let projects = stores(&fake, &["tst", "ctx", "ctx"]);
        let work = fake.work(projects.clone());
        let r = work.resolve("tst-qq9.1").await.unwrap();
        assert_eq!(
            r.to_string(),
            format!("bead:n1:{}#tst-qq9.1", projects[0].name())
        );
        let shared = work.resolve("ctx-1").await.unwrap_err().to_string();
        assert!(shared.contains("could be in"), "{shared}");
        let none = work.resolve("zz-1").await.unwrap_err().to_string();
        assert!(none.contains("no Beads store"), "{none}");
        assert_eq!(fake.runs("bd").len(), 3, "each prefix asked once");
        let written = format!("bead:n1:{}#ctx-1", projects[1].name());
        assert_eq!(work.resolve(&written).await.unwrap().to_string(), written);
        assert_eq!(work.resolve("a/b#2").await.unwrap().to_string(), "gh:a/b#2");
        assert!(work.resolve("not a ref").await.is_err());
    }

    #[tokio::test]
    async fn a_store_whose_prefix_cannot_be_read_stops_resolution() {
        let fake = Fake::new("unreadable", "exit 1", &bd());
        let mut projects = stores(&fake, &["tst"]);
        projects.push(Project::Beads(Store {
            folder: fake.dir.join("no-store").display().to_string(),
        }));
        let error = fake.work(projects).resolve("tst-1").await.unwrap_err();
        assert!(
            format!("{error:#}").contains("no Beads store in"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn reads_one_item_from_either_tracker() {
        let fake = Fake::new("item", &fixture("item.json"), &bd());
        let mut projects = stores(&fake, &["tst", "tst"]);
        projects.push(Project::GitHub(Repo {
            name: "example-owner/parvi".into(),
            checkout: None,
        }));
        let work = fake.work(projects.clone());
        let bead = work
            .item(
                &work
                    .resolve(&format!("bead:n1:{}#tst-qq9.2", projects[0].name()))
                    .await
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bead.title, "Show a failed store as failed");
        assert_eq!(bead.family.blockers.len(), 2);
        assert_eq!(
            work.reference_line(&bead).await,
            format!(
                "bead tst-qq9.2 ({}): Show a failed store as failed",
                projects[0].name()
            )
        );
        let issue = work
            .item(&work.resolve("example-owner/parvi#51").await.unwrap())
            .await
            .unwrap();
        assert_eq!(fake.runs("gh"), vec!["owner=example-owner name=parvi"]);
        assert_eq!(
            work.reference_line(&issue).await,
            "issue example-owner/parvi#51: Example work item"
        );
        for missing in [
            format!("bead:n1:{}#tst-404", projects[0].name()),
            format!("bead:n2:{}#tst-1", projects[0].name()),
            "bead:n1:/elsewhere#tst-1".into(),
        ] {
            let r = parvi_work_ref::parse_resolved(&missing).unwrap();
            assert!(work.item(&r).await.is_err(), "{missing}");
        }
    }
}
