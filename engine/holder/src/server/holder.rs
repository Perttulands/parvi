//! Every session and client of one holder.
//!
//! Lock order: a pane's state, then the console, outboxes and client flags.
//! The session and client maps are only held to look things up or to queue
//! to outboxes, never while taking a pane's state.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use super::client::Client;
use super::log::Log;
use super::session::{Session, Spec};

pub struct Holder {
    pub log: Log,
    pub pid: u32,
    pub start_time: u64,
    sessions: Mutex<BTreeMap<u32, Arc<Session>>>,
    clients: Mutex<BTreeMap<u32, Arc<Client>>>,
    pub buffers: Mutex<HashMap<String, String>>,
    next_session: AtomicU32,
    next_pane: AtomicU32,
    next_client: AtomicU32,
}

impl Holder {
    pub fn new(log: Log) -> Arc<Self> {
        Arc::new(Holder {
            log,
            pid: std::process::id(),
            start_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos() as u64,
            sessions: Mutex::default(),
            clients: Mutex::default(),
            buffers: Mutex::default(),
            next_session: AtomicU32::new(0),
            next_pane: AtomicU32::new(0),
            next_client: AtomicU32::new(0),
        })
    }

    pub fn sessions(&self) -> Vec<Arc<Session>> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }

    pub fn session(&self, id: u32) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    pub fn clients(&self) -> Vec<Arc<Client>> {
        self.clients.lock().unwrap().values().cloned().collect()
    }

    pub fn connect(&self) -> Arc<Client> {
        let client = Arc::new(Client::new(
            self.next_client.fetch_add(1, Ordering::Relaxed),
        ));
        self.clients
            .lock()
            .unwrap()
            .insert(client.id, client.clone());
        client
    }

    pub fn disconnect(&self, client: &Arc<Client>) {
        self.clients.lock().unwrap().remove(&client.id);
        self.detach(client);
        client.outbox.close();
    }

    /// Takes `client` off the session it is attached to, if any.
    pub fn detach(&self, client: &Arc<Client>) {
        let attached = client.flags.lock().unwrap().session.take();
        if let Some(session) = attached.and_then(|id| self.session(id)) {
            let mut state = session.state.lock().unwrap();
            state.viewers.retain(|viewer| viewer.id != client.id);
        }
    }

    /// The session a target names: `%<pane>`, `$<id>` or a name, optionally
    /// followed by `:window.pane`. Without a target, the client's session,
    /// else the first.
    pub fn resolve(&self, client: &Client, target: Option<&str>) -> Result<Arc<Session>, String> {
        let sessions = self.sessions.lock().unwrap();
        let Some(target) = target else {
            let attached = client.flags.lock().unwrap().session;
            return attached
                .and_then(|id| sessions.get(&id))
                .or_else(|| sessions.values().next())
                .cloned()
                .ok_or_else(|| "no sessions".to_owned());
        };
        let found = if let Some(pane) = target.strip_prefix('%') {
            sessions.values().find(|s| pane.parse() == Ok(s.pane))
        } else if let Some(id) = target.strip_prefix('$') {
            id.parse().ok().and_then(|id| sessions.get(&id))
        } else {
            let name = target.split([':', '.']).next().unwrap_or(target);
            sessions.values().find(|s| s.name == name)
        };
        found
            .cloned()
            .ok_or_else(|| format!("can't find session: {target}"))
    }

    /// Starts a session and tells every client the sessions changed.
    /// A session without a name is named by its id, as in tmux.
    pub fn create(self: &Arc<Self>, mut spec: Spec) -> Result<Arc<Session>, String> {
        if spec.name.contains([':', '.']) {
            return Err(format!("invalid session name: {}", spec.name));
        }
        // Held while the program starts, so two sessions can't take one name.
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.values().any(|s| s.name == spec.name) {
            return Err(format!("duplicate session: {}", spec.name));
        }
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        if spec.name.is_empty() {
            spec.name = id.to_string();
        }
        let pane = self.next_pane.fetch_add(1, Ordering::Relaxed);
        let (session, feeds) = Session::start(id, pane, spec).map_err(|e| format!("{e:#}"))?;
        sessions.insert(id, session.clone());
        drop(sessions);
        self.log.line(&format!(
            "session ${id} pane %{pane} pid {} passthrough {} {:?}",
            session.pid, session.passthrough, session.command
        ));
        session.feed(feeds);
        let (holder, waited) = (self.clone(), session.clone());
        std::thread::spawn(move || {
            let code = waited.wait();
            holder.retire(&waited, code);
        });
        self.broadcast("%sessions-changed\n");
        Ok(session)
    }

    /// Forgets a session whose program ended: its viewers get `%exit`, every
    /// client `%sessions-changed`.
    fn retire(&self, session: &Session, code: u32) {
        self.sessions.lock().unwrap().remove(&session.id);
        let viewers = std::mem::take(&mut session.state.lock().unwrap().viewers);
        for viewer in viewers {
            viewer.flags.lock().unwrap().session = None;
            viewer.outbox.push(b"%exit\n".to_vec());
        }
        self.broadcast("%sessions-changed\n");
        self.log
            .line(&format!("session ${} ended with {code}", session.id));
        session.close();
    }

    fn broadcast(&self, line: &str) {
        for client in self.clients.lock().unwrap().values() {
            client.outbox.push(line.as_bytes().to_vec());
        }
    }
}
