//! Local ownership of agent seats through the terminal engine.
use crate::{
    Claude, Codex, Dispatch, HarnessAdapter, Pi, SeatId, SeatRecords, Shell, SlotBinding,
    TranscriptRoots,
};
use parvi_term::{SessionSpec, Sessions, waiting::harness::Harness};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct LocalSeats {
    engine: Arc<dyn Sessions>,
    records: SeatRecords,
    node: String,
    roots: TranscriptRoots,
    creation: Arc<Mutex<()>>,
    pub(super) turns: crate::TurnWorktrees,
    pub(super) running: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    pub(super) runtime: tokio::runtime::Handle,
}
impl LocalSeats {
    pub fn new(
        engine: Arc<dyn Sessions>,
        root: impl Into<PathBuf>,
        node: String,
        roots: TranscriptRoots,
    ) -> Self {
        let root = root.into();
        Self {
            engine,
            turns: crate::TurnWorktrees::new(root.join("turns"), Default::default()),
            records: SeatRecords::new(root),
            node,
            roots,
            creation: Arc::new(Mutex::new(())),
            running: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            runtime: tokio::runtime::Handle::current(),
        }
    }
    /// The checkouts this node makes turn worktrees from, for steps other
    /// coordinators place here.
    pub fn with_repositories(mut self, repositories: BTreeMap<String, PathBuf>) -> Self {
        self.turns = self.turns.with_repositories(repositories);
        self
    }
    pub fn records(&self) -> &SeatRecords {
        &self.records
    }
    pub fn engine(&self) -> &Arc<dyn Sessions> {
        &self.engine
    }
    pub fn roots(&self) -> &TranscriptRoots {
        &self.roots
    }
    /// Create once, or verify the identity of the existing owned seat.
    pub async fn dispatch(&self, request: Dispatch) -> Result<String, String> {
        let _creation = self.creation.lock().await;
        self.records.create(&request).map_err(|e| e.to_string())?;
        let _guard = self.creation_guard(&request.id).await?;
        if let Some(seat) = self.records.seat(&request.id).map_err(|e| e.to_string())? {
            let ended = self
                .records
                .events(&request.id, 0)
                .map_err(|e| e.to_string())?
                .iter()
                .any(|event| event.key == "seat-cleanup");
            let completed = self
                .records
                .result(&request.id)
                .map_err(|e| e.to_string())?
                .is_some();
            if !ended && !completed {
                self.engine
                    .verify(&seat.session)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            self.record_created(&request.id, &seat)?;
            return Ok(request.id);
        }
        let (_, adapter) = adapter(&request.harness)?;
        let shell = if cfg!(windows) {
            Shell::PowerShell
        } else {
            Shell::Posix
        };
        let binding = SlotBinding {
            model: request.model.clone(),
            effort: request.effort.clone(),
        };
        let launch = adapter
            .launch(&binding, &request.cwd, shell, &self.roots)
            .map_err(|e| e.to_string())?;
        // The name is derived from the dispatch, so a session found under it
        // was created by an earlier attempt that crashed before binding it.
        let name = format!(
            "parvi-{}-{}-{}",
            component(&request.run_id),
            component(&request.slot_id),
            &format!("{:x}", Sha256::digest(request.id.as_bytes()))[..8]
        );
        if let Some(session) = self.engine.find(&name).await.map_err(|e| e.to_string())? {
            return self.bind_created(&request.id, name, session).await;
        }
        let mut env = BTreeMap::new();
        if request.harness == "claude-code" {
            // Claude's unstyled ghost text is indistinguishable from an
            // operator's unsent input on a terminal screen.
            env.insert(
                "CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION".into(),
                "false".into(),
            );
        }
        let session = self
            .engine
            .create(SessionSpec {
                name: name.clone(),
                cwd: request.cwd.clone(),
                env,
                command: command(launch),
                cols: 160,
                rows: 48,
            })
            .await
            .map_err(|e| e.to_string())?;
        self.bind_created(&request.id, name, session).await
    }
    async fn bind_created(
        &self,
        id: &str,
        name: String,
        session: parvi_term::SessionRef,
    ) -> Result<String, String> {
        let seat = SeatId {
            node: self.node.clone(),
            session_name: name,
            session,
        };
        if let Err(error) = self.records.bind_seat(id, seat.clone()) {
            let cleanup = self.engine.kill(&seat.session).await;
            return Err(format!(
                "cannot persist owned seat: {error}; cleanup: {cleanup:?}"
            ));
        }
        self.record_created(id, &seat)?;
        Ok(id.to_owned())
    }
    fn record_created(&self, id: &str, seat: &SeatId) -> Result<(), String> {
        let dispatch = self.records.dispatch(id).map_err(|e| e.to_string())?;
        let data = json!({"dispatchId":id,"node":seat.node,"sessionName":seat.session_name,"sessionId":seat.session.session_id,"paneId":seat.session.pane_id,"generation":seat.session.generation,"harness":dispatch.harness,"model":dispatch.model,"effort":dispatch.effort});
        self.records
            .append(
                id,
                "seat-created",
                "seat_created",
                data.as_object().cloned().unwrap_or_default(),
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Bind a correction dispatch to the same live seat.
    pub async fn reuse(&self, request: &Dispatch, original: &str) -> Result<(), String> {
        let _creation = self.creation.lock().await;
        self.records.create(request).map_err(|e| e.to_string())?;
        let _guard = self.creation_guard(&request.id).await?;
        let seat = self
            .records
            .seat(original)
            .map_err(|e| e.to_string())?
            .ok_or("original dispatch has no seat")?;
        self.engine
            .verify(&seat.session)
            .await
            .map_err(|e| e.to_string())?;
        self.records
            .bind_seat(&request.id, seat)
            .map_err(|e| e.to_string())
    }
    /// End only the recorded server generation; retries return the first outcome.
    pub async fn end_seat(&self, dispatch_id: &str, cause: &str) -> Result<String, String> {
        let _creation = self.creation.lock().await;
        let _guard = self.creation_guard(dispatch_id).await?;
        if let Some(event) = self
            .records
            .events(dispatch_id, 0)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|event| event.key == "seat-cleanup")
        {
            return Ok(event
                .data
                .get("outcome")
                .and_then(|value| value.as_str())
                .unwrap_or("left_cleanup_failed")
                .into());
        }
        let seat = self
            .records
            .seat(dispatch_id)
            .map_err(|e| e.to_string())?
            .ok_or("dispatch has no seat")?;
        let result = self.engine.kill(&seat.session).await;
        let (outcome, detail) = match result {
            Ok(()) => ("ended", String::new()),
            Err(error) if error.to_string().contains("generation") => {
                ("left_socket_changed", error.to_string())
            }
            Err(error) => ("left_cleanup_failed", error.to_string()),
        };
        let data = json!({"dispatchId":dispatch_id,"outcome":outcome,"cause":cause,"detail":detail,"sessionName":seat.session_name});
        self.records
            .append(
                dispatch_id,
                "seat-cleanup",
                "seat_cleanup",
                data.as_object().cloned().unwrap_or_default(),
            )
            .map_err(|e| e.to_string())?;
        Ok(outcome.into())
    }
    pub(super) fn detach(&self, dispatch_id: &str) -> Result<String, String> {
        let seat = self
            .records
            .seat(dispatch_id)
            .map_err(|e| e.to_string())?
            .ok_or("dispatch has no seat")?;
        let data = json!({"dispatchId":dispatch_id,"outcome":"left_shutdown","cause":"shutdown","sessionName":seat.session_name});
        self.records
            .append(
                dispatch_id,
                "seat-detached",
                "seat_cleanup",
                data.as_object().cloned().unwrap_or_default(),
            )
            .map_err(|e| e.to_string())?;
        Ok("left_shutdown".into())
    }
    async fn creation_guard(&self, id: &str) -> Result<std::fs::File, String> {
        let records = self.records.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || records.creation_guard(&id))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    }
}
/// What the seat's session runs. On Windows the harness runs in PowerShell,
/// which finds npm's `.cmd` and `.ps1` shims as a person's prompt would;
/// the command is passed encoded, so no quoting layer can split it.
fn command(launch: crate::Launch) -> Vec<String> {
    if !cfg!(windows) {
        return launch.argv;
    }
    let utf16: Vec<u8> = launch
        .command
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    vec![
        "powershell.exe".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        base64(&utf16),
    ]
}
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
pub(crate) fn adapter(name: &str) -> Result<(Harness, Arc<dyn HarnessAdapter>), String> {
    match name {
        "claude-code" => Ok((Harness::Claude, Arc::new(Claude))),
        "openai-codex" => Ok((Harness::Codex, Arc::new(Codex))),
        "pi" => Ok((Harness::Pi, Arc::new(Pi))),
        _ => Err(format!("unknown harness {name:?}")),
    }
}
fn component(text: &str) -> String {
    let value: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if value.is_empty() {
        "seat".into()
    } else {
        value
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn encodes_base64_with_padding() {
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"M"), "TQ==");
        assert_eq!(super::base64(b""), "");
    }
}
