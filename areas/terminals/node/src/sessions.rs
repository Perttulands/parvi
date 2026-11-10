//! The terminals area as other areas reach it ([`parvi_term::Sessions`]).

use std::sync::Arc;

use anyhow::Context;
use parvi_term::backend::Reply;
use parvi_term::control::PaneId;
use parvi_term::engine::{Delivering, PaneStream, SessionInfo};
use parvi_term::{Renames, Screen, SessionRef, SessionSpec, Sessions, Start};
use tokio::sync::watch;

use crate::hub::Hub;
use crate::launch::{Config, Harness, Launch};
use crate::launcher::Launcher;

pub(crate) struct Provided {
    pub hub: Arc<Hub>,
    pub launcher: Arc<Launcher>,
}

impl Sessions for Provided {
    fn list(&self) -> Vec<SessionInfo> {
        self.hub.sessions()
    }

    fn users(&self) -> Vec<String> {
        self.hub.users()
    }

    fn watch(&self) -> watch::Receiver<u64> {
        self.hub.watch()
    }

    fn renames(&self) -> Vec<(Option<String>, Renames)> {
        self.hub.renames()
    }

    /// CHROTE's pane for an unattended send.
    fn active_pane<'a>(&'a self, session: &'a str) -> Reply<'a, PaneId> {
        Box::pin(async move {
            let listed = self.hub.sessions();
            let info = listed
                .iter()
                .find(|s| s.id == session)
                .with_context(|| format!("no session {session}"))?;
            if let [(pane, _, _)] = info.panes.as_slice() {
                return Ok(*pane);
            }
            let (server, id) = self.hub.session(session)?;
            let panes = server
                .backend()
                .command(&[
                    "list-panes",
                    "-s",
                    "-t",
                    id,
                    "-F",
                    "#{pane_id} #{window_active} #{pane_active}",
                ])
                .await
                .with_context(|| info.name.clone())?;
            let pane = active(&panes).with_context(|| {
                format!(
                    "{} has {} panes and no active one to send to",
                    info.name,
                    info.panes.len()
                )
            })?;
            self.hub
                .pane_id(server, pane)
                .with_context(|| format!("{}: pane %{pane} is out of range", info.name))
        })
    }

    fn local_pane(&self, pane: PaneId) -> PaneId {
        self.hub.pane(pane).map_or(pane, |(_, id)| id)
    }

    fn deliver(&self, pane: PaneId, text: String, enter: bool) -> Reply<'_, Delivering> {
        Box::pin(self.hub.deliver(pane, text, enter))
    }

    fn start(&self, start: Start) -> Reply<'_, String> {
        Box::pin(async move {
            let launcher = self.launcher.get();
            let config = Config {
                harnesses: vec![Harness {
                    id: "start".into(),
                    command: start.command,
                    flags: String::new(),
                }],
                folders: Vec::new(),
                cols: launcher.cols,
                rows: launcher.rows,
            };
            let launch = Launch {
                harness: "start".into(),
                folder: start.folder,
                name: start.name,
                flags: String::new(),
                user: start.user,
                notify: false,
                size: None,
            };
            let (session, _) = self.hub.launch(&config, &launch).await?;
            Ok(session)
        })
    }

    fn create(&self, spec: SessionSpec) -> Reply<'_, SessionRef> {
        Box::pin(async move { self.hub.own_engine().create_session(spec).await })
    }

    fn find<'a>(&'a self, name: &'a str) -> Reply<'a, Option<SessionRef>> {
        Box::pin(async move { self.hub.own_engine().find_session(name).await })
    }

    fn verify<'a>(&'a self, seat: &'a SessionRef) -> Reply<'a, ()> {
        Box::pin(async move { self.hub.own_engine().verify_session(seat).await })
    }

    fn kill<'a>(&'a self, seat: &'a SessionRef) -> Reply<'a, ()> {
        Box::pin(async move { self.hub.own_engine().kill_session(seat).await })
    }

    fn follow(&self, pane: PaneId) -> Option<PaneStream> {
        self.hub.subscribe(pane)
    }

    fn screen(&self, pane: PaneId) -> Option<Screen> {
        let (server, local) = self.hub.pane(pane)?;
        server.engine.read_screen(local)
    }

    fn input(&self, pane: PaneId, bytes: Vec<u8>) -> Reply<'_, ()> {
        Box::pin(self.hub.send_input(pane, bytes))
    }

    fn pid(&self, pane: PaneId) -> Reply<'_, u32> {
        Box::pin(async move {
            let (server, local) = self.hub.pane(pane).context("unknown pane")?;
            let target = format!("%{local}");
            let pid = server
                .backend()
                .command(&["display-message", "-p", "-t", &target, "#{pane_pid}"])
                .await?;
            pid.trim()
                .parse()
                .with_context(|| format!("pane {pane} has no process id: {pid:?}"))
        })
    }
}

/// The active pane of the active window, from `list-panes -s` lines of
/// `#{pane_id} #{window_active} #{pane_active}`.
fn active(listed: &str) -> Option<PaneId> {
    listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let pane = parts.next()?.strip_prefix('%')?.parse().ok()?;
        (parts.next()? == "1" && parts.next()? == "1").then_some(pane)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 13: the first pane was used, so a shell beside the agent got
    /// the prompt and its Enter.
    #[test]
    fn the_active_pane_of_the_active_window_is_chosen() {
        let listed = "%3 0 1\n%4 1 0\n%5 1 1\n%6 0 0\n";
        assert_eq!(active(listed), Some(5));
        assert_eq!(active("%3 0 1\n%4 1 0\n"), None, "none clear: refused");
        assert_eq!(active(""), None);
    }
}
