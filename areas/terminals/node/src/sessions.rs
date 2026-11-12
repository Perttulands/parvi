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

    fn listed(&self) -> Reply<'_, Vec<SessionInfo>> {
        Box::pin(async move {
            self.hub.ready().await?;
            Ok(self.hub.sessions())
        })
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
        Box::pin(self.hub.active_pane(session))
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
