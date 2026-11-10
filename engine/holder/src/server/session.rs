//! One session: a pseudo console with one pane, its screen, and the threads
//! that feed it.
//!
//! The pane's state lock orders everything a client sees of it: output is
//! applied to the screen and queued to viewers under it, and replies that read
//! the screen are queued under it too. So, as with tmux, anything queued
//! before a `capture-pane` reply is already in the capture.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::client::{Client, now};
use crate::input::{self, Input};
use crate::output;
use crate::queries::Queries;
use crate::screen::{self, clamp};
use crate::sys::conpty::{self, Console, Process};

/// What a new session runs and how big it starts.
pub struct Spec {
    pub name: String,
    pub command: String,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub pinned: bool,
    pub cols: u16,
    pub rows: u16,
}

pub struct Session {
    pub id: u32,
    pub pane: u32,
    pub name: String,
    pub command: String,
    /// The folder it started in; a console's later `cd` is not seen.
    pub cwd: Option<String>,
    pub created: u64,
    pub pid: u32,
    pub passthrough: bool,
    pub pinned: bool,
    pub state: Mutex<PaneState>,
    console: Mutex<Option<Console>>,
    process: Process,
    input: Arc<Input>,
}

pub struct PaneState {
    pub screen: vt100::Parser,
    pub activity: u64,
    /// The clients attached to this session.
    pub viewers: Vec<Arc<Client>>,
}

/// The parts of a started session its threads own.
pub struct Feeds {
    output: File,
    input: File,
}

impl Session {
    pub fn start(id: u32, pane: u32, spec: Spec) -> Result<(Arc<Session>, Feeds)> {
        let (cols, rows) = clamp(spec.cols, spec.rows);
        let spawned = conpty::spawn(&spec.command, spec.cwd.as_deref(), &spec.env, cols, rows)?;
        let session = Arc::new(Session {
            id,
            pane,
            name: spec.name,
            command: spec.command,
            cwd: spec.cwd,
            created: now(),
            pid: spawned.pid,
            passthrough: spawned.passthrough,
            pinned: spec.pinned,
            state: Mutex::new(PaneState {
                screen: screen::new(cols, rows),
                activity: now(),
                viewers: Vec::new(),
            }),
            console: Mutex::new(Some(spawned.console)),
            process: spawned.process,
            input: Arc::new(Input::new(input::LIMIT)),
        });
        let feeds = Feeds {
            output: spawned.output,
            input: spawned.input,
        };
        Ok((session, feeds))
    }

    /// Starts the threads that write the pane's input and read its output.
    pub fn feed(self: &Arc<Self>, feeds: Feeds) {
        let (mut console, input) = (feeds.input, self.input.clone());
        std::thread::spawn(move || {
            // Writes block while the program isn't reading, so they happen
            // here and never under a lock.
            while let Some(bytes) = input.next() {
                if console.write_all(&bytes).is_err() {
                    break;
                }
                input.written(bytes.len());
            }
        });
        let session = self.clone();
        std::thread::spawn(move || session.pump(feeds.output));
    }

    /// Queues bytes for the program to read; refused once the session ended
    /// or while the program isn't reading what it has.
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.input.push(bytes)
    }

    /// Reads the pane's output for as long as the console lives.
    fn pump(&self, mut output: File) {
        let mut buf = vec![0u8; 64 * 1024];
        let mut queries = Queries::default();
        loop {
            let n = match output.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let data = &buf[..n];
            let mut state = self.state.lock().unwrap();
            state.activity = now();
            if self.passthrough {
                let reply = queries.answer(&mut state.screen, data);
                if !reply.is_empty() {
                    // A program that isn't reading won't miss the answer.
                    let _ = self.write(reply);
                }
            } else {
                state.screen.process(data);
            }
            if !state.viewers.is_empty() {
                let line = output::line(self.pane, data);
                for viewer in &state.viewers {
                    viewer.outbox.output(self.pane, &line);
                }
            }
        }
    }

    /// Blocks until the program ends, then ends whatever it left running.
    pub fn wait(&self) -> u32 {
        let code = self.process.wait();
        self.process.kill();
        code
    }

    pub fn kill(&self) {
        self.process.kill();
    }

    /// Closes the console once the program is gone. Blocks until its output
    /// is drained.
    pub fn close(&self) {
        self.input.close();
        let console = self.console.lock().unwrap().take();
        if let Some(console) = console {
            console.close();
        }
    }

    /// Resizes the pane and tells its viewers, if the size changes.
    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = clamp(cols, rows);
        let mut state = self.state.lock().unwrap();
        if state.screen.screen().size() == (rows, cols) {
            return;
        }
        if let Some(console) = &*self.console.lock().unwrap() {
            console.resize(cols, rows);
        }
        state.screen.screen_mut().set_size(rows, cols);
        let layout = format!(
            "%layout-change @{0} {cols}x{rows} {cols}x{rows} *\n",
            self.id
        );
        for viewer in &state.viewers {
            viewer.outbox.push(layout.clone().into_bytes());
        }
    }

    /// A format variable of this session, its pane, or `client`.
    pub fn var(&self, state: &PaneState, client: Option<&Client>, name: &str) -> Option<String> {
        let screen = state.screen.screen();
        let (rows, cols) = screen.size();
        let (cursor_y, cursor_x) = screen.cursor_position();
        Some(match name {
            "session_id" => format!("${}", self.id),
            "session_name" => self.name.clone(),
            "session_windows" => "1".into(),
            "session_attached" => state.viewers.len().to_string(),
            "session_created" => self.created.to_string(),
            "session_activity" | "window_activity" => state.activity.to_string(),
            "window_id" => format!("@{}", self.id),
            "window_index" | "pane_index" => "0".into(),
            "window-size" => if self.pinned { "manual" } else { "latest" }.into(),
            "mouse" => "0".into(),
            "pane_id" => format!("%{}", self.pane),
            "pane_width" => cols.to_string(),
            "pane_height" => rows.to_string(),
            "pane_pid" => self.pid.to_string(),
            "pane_current_command" => self.command.split(' ').next()?.to_owned(),
            "pane_current_path" => self.cwd.clone()?,
            "pane_passthrough" => u8::from(self.passthrough).to_string(),
            "cursor_x" => cursor_x.to_string(),
            "cursor_y" => cursor_y.to_string(),
            "cursor_flag" => u8::from(!screen.hide_cursor()).to_string(),
            "alternate_on" => u8::from(screen.alternate_screen()).to_string(),
            "client_name" => client?.name.clone(),
            "client_flags" => client?.flag_list(),
            "user" => std::env::var("USERNAME").ok()?,
            _ => return None,
        })
    }
}
