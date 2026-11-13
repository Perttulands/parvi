//! Scheduled tasks as each node keeps them (`/api/scheduled-tasks`): what
//! the page has read, selected and is editing. A form that has changed is
//! left only by asking twice within three seconds, as CHROTE's.

use std::collections::BTreeMap;

use parvi_scheduled_contract::{Envelope, RunEntry, SUCCESS, Task};
use serde::Deserialize;

use crate::form::TaskForm;

/// How long a second ask to discard is taken as meaning it.
pub const CONFIRM_MS: u64 = 3_000;

/// One node's tasks as last read.
#[derive(Default)]
pub struct NodeTasks {
    pub list: Option<Vec<Task>>,
    pub error: Option<String>,
    pub reading: bool,
    /// An invalidation arrived while the current read was in flight.
    pub reread: bool,
}

#[derive(Default)]
pub struct Scheduled {
    pub nodes: BTreeMap<String, NodeTasks>,
    /// (node, task id).
    pub selected: Option<(String, String)>,
    pub form: Option<TaskForm>,
    pub dirty: bool,
    /// When a discard was asked for once, Unix ms.
    pub discard_armed: Option<u64>,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Deserialize)]
struct Tasks {
    tasks: Vec<Task>,
}

#[derive(Deserialize)]
pub struct Saved {
    pub task: Task,
    pub run: Option<RunEntry>,
}

/// The data of a CHROTE envelope, or the error's words.
pub fn data<T: serde::de::DeserializeOwned>(status: u16, body: &[u8]) -> Result<T, String> {
    match serde_json::from_slice::<Envelope>(body) {
        Ok(Envelope {
            success: true,
            data: Some(d),
            ..
        }) => serde_json::from_value(d).map_err(|e| format!("The node's answer did not read: {e}")),
        Ok(Envelope { error: Some(e), .. }) => Err(e.message),
        _ => {
            let text = String::from_utf8_lossy(body).trim().to_string();
            Err(if text.is_empty() {
                format!("The node answered {status}.")
            } else {
                text
            })
        }
    }
}

impl NodeTasks {
    pub fn listed(&mut self, status: u16, body: &[u8]) {
        self.reading = false;
        match data::<Tasks>(status, body) {
            Ok(t) => {
                self.list = Some(t.tasks);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// A task the node answered with replaces its row, or joins the list.
    pub fn put(&mut self, task: Task) {
        let list = self.list.get_or_insert_default();
        match list.iter_mut().find(|t| t.id == task.id) {
            Some(row) => *row = task,
            None => list.push(task),
        }
    }
}

impl Scheduled {
    pub fn task(&self, node: &str, id: &str) -> Option<&Task> {
        self.nodes
            .get(node)?
            .list
            .as_ref()?
            .iter()
            .find(|t| t.id == id)
    }

    pub fn selected_task(&self) -> Option<(&str, &Task)> {
        let (node, id) = self.selected.as_ref()?;
        Some((node.as_str(), self.task(node, id)?))
    }

    /// Whether the form may be left now. A changed form asks first: the
    /// same gesture again within three seconds discards it.
    pub fn may_leave(&mut self, now: u64) -> bool {
        if self.form.is_none() || !self.dirty {
            return true;
        }
        if self
            .discard_armed
            .is_some_and(|at| now.saturating_sub(at) < CONFIRM_MS)
        {
            self.discard_armed = None;
            return true;
        }
        self.discard_armed = Some(now);
        self.notice = Some(
            "Unsaved scheduled task changes. Repeat within three seconds to discard them.".into(),
        );
        false
    }

    pub fn open(&mut self, form: TaskForm, now: u64) -> bool {
        if !self.may_leave(now) {
            return false;
        }
        self.form = Some(form);
        self.dirty = false;
        self.notice = None;
        self.error = None;
        true
    }

    pub fn close(&mut self, now: u64) {
        if self.may_leave(now) {
            self.form = None;
            self.dirty = false;
        }
    }

    pub fn change(&mut self, f: impl FnOnce(&mut TaskForm)) {
        if let Some(form) = &mut self.form {
            f(form);
            self.dirty = true;
            self.notice = None;
        }
    }

    /// The words after a Run now, CHROTE's.
    pub fn sent_words(run: &RunEntry, targets: usize) -> String {
        let total = if run.targets.is_empty() {
            targets
        } else {
            run.targets.len()
        };
        let delivered = run.targets.iter().filter(|t| t.status == SUCCESS).count();
        if run.status == SUCCESS {
            format!(
                "Sent to {total} {}.",
                if total == 1 { "session" } else { "sessions" }
            )
        } else {
            let why = if run.message.is_empty() {
                "delivery failed"
            } else {
                &run.message
            };
            format!("Sent to {delivered} of {total}: {why}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::TaskForm;

    #[test]
    fn a_changed_form_is_left_only_when_asked_twice() {
        let mut s = Scheduled::default();
        assert!(s.open(TaskForm::new("n", vec![]), 0));
        s.change(|f| f.prompt = "x".into());
        assert!(!s.open(TaskForm::new("n", vec![]), 1_000));
        assert!(s.notice.as_deref().unwrap().contains("Repeat"));
        assert!(s.open(TaskForm::new("n", vec![]), 2_000), "asked twice");
        s.change(|f| f.prompt = "y".into());
        assert!(!s.open(TaskForm::new("n", vec![]), 10_000));
        assert!(!s.open(TaskForm::new("n", vec![]), 14_000), "too late");
    }

    #[test]
    fn envelopes_give_their_data_or_the_errors_words() {
        let refused =
            br#"{"success":false,"error":{"code":"BAD_REQUEST","message":"invalid scheduled task: prompt is required"},"timestamp":"x"}"#;
        assert_eq!(
            data::<Tasks>(400, refused).err().as_deref(),
            Some("invalid scheduled task: prompt is required")
        );
        let mut n = NodeTasks::default();
        n.listed(200, br#"{"success":true,"data":{"tasks":[{"id":"tsk_1","name":"a","prompt":"p","targets":[{"sessionName":"ops"}],"schedule":{"type":"interval","everyMinutes":5,"timezone":"UTC"},"enabled":true,"paused":false,"nextRun":"2026-06-27T14:05:00Z","createdAt":"2026-06-27T14:00:00Z","updatedAt":"2026-06-27T14:00:00Z"}]},"timestamp":"x"}"#);
        assert_eq!(n.list.as_ref().unwrap()[0].targets[0].session_name, "ops");
    }
}
