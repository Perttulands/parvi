//! The tmux commands the holder answers: the subset Parvi's engine, launcher
//! and search send, with tmux's flags and replies.
//!
//! A handler answers through its [`Reply`] and returns the [`Sent`] only
//! `Reply::send` makes, so every command is answered exactly once. Handlers
//! that read a pane answer while holding its state, which keeps the reply in
//! order with the pane's output.

use std::sync::Arc;
use std::time::Duration;

use super::client::Client;
use super::holder::Holder;
use super::session::{Session, Spec};
use crate::format::expand;
use crate::guard;
use crate::keys;
use crate::screen::capture;
use crate::words::Args;

/// What a session runs when `new-session` names nothing: tmux runs the
/// default shell, and every Windows has this one.
const DEFAULT_COMMAND: &str = "powershell.exe -NoLogo";

pub struct Reply<'a> {
    client: &'a Client,
    number: u64,
}

#[must_use]
pub struct Sent(());

impl Reply<'_> {
    pub fn send(self, result: Result<String, String>) -> Sent {
        self.client.reply(self.number, result);
        Sent(())
    }
}

/// Runs command `number` from `client`.
pub fn run(holder: &Arc<Holder>, client: &Arc<Client>, number: u64, words: &[String]) -> bool {
    let reply = Reply { client, number };
    let Some((name, rest)) = words.split_first() else {
        let _ = reply.send(Err("empty command".into()));
        return false;
    };
    let cx = Context { holder, client };
    if name == "if-shell" {
        return cx.guarded_submit(rest, reply);
    }
    let _ = match name.as_str() {
        "attach-session" | "attach" => cx.attach(rest, reply),
        "capture-pane" | "capturep" => cx.capture(rest, reply),
        "display-message" | "display" => cx.display(rest, reply),
        "has-session" | "has" => {
            let a = Args::parse(rest, "t");
            reply.send(cx.target(&a).map(|_| String::new()))
        }
        "kill-server" => cx.kill_server(reply),
        "kill-session" | "kill-ses" => {
            let a = Args::parse(rest, "t");
            reply.send(cx.target(&a).map(|s| {
                // The session leaves once its program has gone.
                s.kill();
                String::new()
            }))
        }
        "list-clients" | "lsc" => cx.list_clients(rest, reply),
        "list-panes" | "lsp" => cx.list_panes(rest, reply),
        "list-sessions" | "ls" => cx.list_sessions(rest, reply),
        "new-session" | "new" => cx.new_session(rest, reply),
        "parvi-create" => cx.create_owned(rest, reply),
        "parvi-kill" => cx.kill_owned(rest, reply),
        "paste-buffer" | "pasteb" => cx.paste(rest, reply),
        "refresh-client" | "refresh" => cx.refresh(rest, reply),
        "send-keys" | "send" => cx.send_keys(rest, reply),
        "set-buffer" | "setb" => {
            let a = Args::parse(rest, "bn");
            let name = a.get('b').unwrap_or("buffer0").to_owned();
            holder
                .buffers
                .lock()
                .unwrap()
                .insert(name, a.rest.join(" "));
            reply.send(Ok(String::new()))
        }
        other => reply.send(Err(format!("unknown command: {other}"))),
    };
    false
}

struct Context<'a> {
    holder: &'a Arc<Holder>,
    client: &'a Arc<Client>,
}

impl Context<'_> {
    fn target(&self, a: &Args) -> Result<Arc<Session>, String> {
        self.holder.resolve(self.client, a.get('t'))
    }

    /// A guarded submit emits a reply for `if-shell` followed by one for its
    /// chosen branch, before any later command on this connection. A holder
    /// never respawns or reuses a pane, so the resolved session and its pid
    /// cannot change between comparison and queuing Enter.
    fn guarded_submit(&self, words: &[String], reply: Reply) -> bool {
        let guard = match guard::parse(words) {
            Ok(guard) => guard,
            Err(error) => {
                let _ = reply.send(Err(error));
                return false;
            }
        };
        let target = format!("%{}", guard.pane);
        let session = match self.holder.resolve(self.client, Some(&target)) {
            Ok(session) => session,
            Err(error) => {
                let _ = reply.send(Err(error));
                return false;
            }
        };
        let number = reply.number;
        let _ = reply.send(Ok(String::new()));
        let result = if session.pid == guard.pid {
            session.write(keys::bytes("Enter")).map(|()| String::new())
        } else {
            Ok("target-changed".into())
        };
        self.client.reply(number + 1, result);
        true
    }

    /// Expands `format` for one session, reading its pane under its lock.
    fn expand_for(&self, session: &Session, client: Option<&Client>, format: &str) -> String {
        let state = session.state.lock().unwrap();
        expand(format, |name| session.var(&state, client, name))
    }

    /// `attach-session [-E] [-f flags] -t <target>`: from here on the client
    /// gets the session's output and notifications.
    fn attach(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tfc");
        let session = match self.target(&a) {
            Ok(session) => session,
            Err(e) => return reply.send(Err(e)),
        };
        self.holder.detach(self.client);
        if let Some(flags) = a.get('f') {
            self.set_flags(self.client, flags);
        }
        let mut state = session.state.lock().unwrap();
        self.client.flags.lock().unwrap().session = Some(session.id);
        state.viewers.push(self.client.clone());
        reply.send(Ok(String::new()))
    }

    fn capture(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tSEb");
        let session = match self.target(&a) {
            Ok(session) => session,
            Err(e) => return reply.send(Err(e)),
        };
        let mut state = session.state.lock().unwrap();
        let text = capture(
            &mut state.screen,
            a.get('S') == Some("-"),
            a.has('e'),
            a.has('J'),
        );
        reply.send(Ok(text))
    }

    fn display(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tcF");
        let session = match self.target(&a) {
            Ok(session) => session,
            Err(e) => return reply.send(Err(e)),
        };
        let format = a.get('F').map_or_else(|| a.rest.join(" "), str::to_owned);
        let state = session.state.lock().unwrap();
        let text = expand(&format, |name| match name {
            "pid" => Some(self.holder.pid.to_string()),
            "start_time" => Some(self.holder.start_time.to_string()),
            _ => session.var(&state, Some(self.client), name),
        });
        reply.send(Ok(text))
    }

    fn list_sessions(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "Ff");
        let format = a.get('F').unwrap_or("#{session_name}: 1 windows");
        let lines: Vec<String> = (self.holder.sessions().iter())
            .map(|s| self.expand_for(s, Some(self.client), format))
            .collect();
        reply.send(Ok(lines.join("\n")))
    }

    /// One pane per session, so `-s` and plain list the same.
    fn list_panes(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tFf");
        let result = self.target(&a).map(|session| {
            self.expand_for(
                &session,
                Some(self.client),
                a.get('F').unwrap_or("#{pane_id}"),
            )
        });
        reply.send(result)
    }

    /// The attached clients, of one session with `-t`.
    fn list_clients(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tFf");
        let only = match a.get('t').map(|_| self.target(&a)).transpose() {
            Ok(only) => only.map(|s| s.id),
            Err(e) => return reply.send(Err(e)),
        };
        let format = a.get('F').unwrap_or("#{client_name}");
        let mut lines = Vec::new();
        for client in self.holder.clients() {
            let attached = client.flags.lock().unwrap().session;
            let Some(id) = attached.filter(|id| only.is_none_or(|only| only == *id)) else {
                continue;
            };
            if let Some(session) = self.holder.session(id) {
                lines.push(self.expand_for(&session, Some(&client), format));
            }
        }
        reply.send(Ok(lines.join("\n")))
    }

    /// `new-session [-d] [-P [-F format]] [-s name] [-c dir] [-x cols] [-y rows] [command...]`.
    fn new_session(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "sFcxynt");
        let size = |flag, default| a.get(flag).and_then(|v| v.parse().ok()).unwrap_or(default);
        let command = if a.rest.is_empty() {
            DEFAULT_COMMAND.to_owned()
        } else {
            a.rest.join(" ")
        };
        let spec = Spec {
            name: a.get('s').unwrap_or_default().to_owned(),
            command,
            cwd: a.get('c').map(str::to_owned),
            env: Vec::new(),
            pinned: false,
            cols: size('x', 80),
            rows: size('y', 24),
        };
        let result = self.holder.create(spec).map(|session| {
            if a.has('P') {
                let format = a.get('F').unwrap_or("#{session_name}:");
                self.expand_for(&session, Some(self.client), format)
            } else {
                String::new()
            }
        });
        reply.send(result)
    }

    /// Owned sessions use a separate command so all fields arrive as data.
    fn create_owned(&self, words: &[String], reply: Reply) -> Sent {
        let result = (|| -> Result<String, String> {
            if words.len() != 6 {
                return Err("invalid owned session spec".into());
            }
            let command: Vec<String> =
                serde_json::from_str(&words[4]).map_err(|e| format!("invalid command: {e}"))?;
            let env: Vec<(String, String)> =
                serde_json::from_str(&words[5]).map_err(|e| format!("invalid environment: {e}"))?;
            if command.is_empty()
                || command[0].is_empty()
                || command.iter().any(|s| s.contains('\0'))
                || env
                    .iter()
                    .any(|(k, v)| k.is_empty() || k.contains(['=', '\0']) || v.contains('\0'))
            {
                return Err("invalid command or environment".into());
            }
            let cols: u16 = words[2].parse().map_err(|_| "invalid columns")?;
            let rows: u16 = words[3].parse().map_err(|_| "invalid rows")?;
            if cols == 0 || cols > 500 || rows == 0 || rows > 200 {
                return Err("invalid session size".into());
            }
            let spec = Spec {
                name: words[0].clone(),
                cwd: Some(words[1].clone()),
                cols,
                rows,
                env,
                pinned: true,
                command: command
                    .iter()
                    .map(|s| windows_arg(s))
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let session = self.holder.create(spec)?;
            Ok(format!(
                "${} %{} {} {}",
                session.id, session.pane, self.holder.pid, self.holder.start_time
            ))
        })();
        reply.send(result)
    }

    fn kill_owned(&self, words: &[String], reply: Reply) -> Sent {
        let result = (|| -> Result<String, String> {
            if words.len() != 3
                || words[1].parse::<u32>() != Ok(self.holder.pid)
                || words[2].parse::<u64>() != Ok(self.holder.start_time)
            {
                return Err("server generation changed".into());
            }
            let id = words[0]
                .strip_prefix('$')
                .and_then(|id| id.parse().ok())
                .ok_or("invalid session id")?;
            let session = self.holder.session(id).ok_or("unknown session")?;
            session.kill();
            Ok(String::new())
        })();
        reply.send(result)
    }

    /// `paste-buffer [-p] [-d] [-b name] [-t target]`: line feeds become
    /// carriage returns, and `-p` brackets the paste if the program asked.
    fn paste(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "bts");
        let name = a.get('b').unwrap_or("buffer0");
        let text = {
            let mut buffers = self.holder.buffers.lock().unwrap();
            if a.has('d') {
                buffers.remove(name)
            } else {
                buffers.get(name).cloned()
            }
        };
        let Some(text) = text else {
            return reply.send(Err(format!("no buffer {name}")));
        };
        let session = match self.target(&a) {
            Ok(session) => session,
            Err(e) => return reply.send(Err(e)),
        };
        let state = session.state.lock().unwrap();
        let bracketed = a.has('p') && state.screen.screen().bracketed_paste();
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        let written = session.write(bytes);
        drop(state);
        reply.send(written.map(|()| String::new()))
    }

    fn send_keys(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tN");
        let session = match self.target(&a) {
            Ok(session) => session,
            Err(e) => return reply.send(Err(e)),
        };
        let bytes = if a.has('H') {
            match keys::hex(&a.rest) {
                Ok(bytes) => bytes,
                Err(e) => return reply.send(Err(e)),
            }
        } else if a.has('l') {
            a.rest.concat().into_bytes()
        } else {
            a.rest.iter().flat_map(|w| keys::bytes(w)).collect()
        };
        let written = session.write(bytes);
        reply.send(written.map(|()| String::new()))
    }

    /// `refresh-client [-t client] [-f flags] [-C size] [-A '%pane:continue']`.
    fn refresh(&self, rest: &[String], reply: Reply) -> Sent {
        let a = Args::parse(rest, "tCfAB");
        let target = match a.get('t') {
            None => self.client.clone(),
            Some(name) => match self.holder.clients().into_iter().find(|c| c.name == name) {
                Some(client) => client,
                None => return reply.send(Err(format!("can't find client: {name}"))),
            },
        };
        if let Some(flags) = a.get('f') {
            self.set_flags(&target, flags);
        }
        if let Some(size) = a.get('C') {
            let parsed = size
                .split_once([',', 'x'])
                .and_then(|(c, r)| Some((c.parse().ok()?, r.parse().ok()?)));
            let Some(size) = parsed else {
                return reply.send(Err(format!("bad size: {size}")));
            };
            target.flags.lock().unwrap().size = Some(size);
            self.apply_size(&target);
        }
        if let Some(action) = a.get('A') {
            let pane = action
                .strip_suffix(":continue")
                .and_then(|p| p.strip_prefix('%'))
                .and_then(|p| p.parse().ok());
            match pane {
                Some(pane) => target.outbox.resume(pane),
                None => return reply.send(Err(format!("unsupported action: {action}"))),
            }
        }
        reply.send(Ok(String::new()))
    }

    /// Applies tmux client flags: `ignore-size`, `pause-after=<seconds>`
    /// and their `!` forms. Others are accepted and ignored, as the holder
    /// has no use for them.
    fn set_flags(&self, client: &Arc<Client>, list: &str) {
        for flag in list.split(',') {
            let (on, flag) = match flag.strip_prefix('!') {
                Some(flag) => (false, flag),
                None => (true, flag),
            };
            let mut flags = client.flags.lock().unwrap();
            if flag == "ignore-size" {
                flags.ignore_size = on;
                drop(flags);
                self.apply_size(client);
            } else if flag.split('=').next() == Some("pause-after") {
                let after = flag
                    .split_once('=')
                    .and_then(|(_, n)| n.parse().ok())
                    .map(Duration::from_secs)
                    .filter(|_| on);
                flags.pause_after = after;
                client.outbox.set_pause_after(after);
            }
        }
    }

    /// A client that sizes its session (no `ignore-size`) and has a size
    /// resizes the session to it.
    fn apply_size(&self, client: &Client) {
        let (session, size) = {
            let flags = client.flags.lock().unwrap();
            (flags.session.filter(|_| !flags.ignore_size), flags.size)
        };
        if let (Some(id), Some((cols, rows))) = (session, size)
            && let Some(session) = self.holder.session(id)
            && !session.pinned
        {
            session.resize(cols, rows);
        }
    }

    /// Ends every session, answers, and exits once the answer is out.
    fn kill_server(&self, reply: Reply) -> Sent {
        for session in self.holder.sessions() {
            session.kill();
        }
        self.holder.log.line("kill-server");
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            std::process::exit(0);
        });
        reply.send(Ok(String::new()))
    }
}

/// Quote a single argument according to the Windows C runtime rules.
fn windows_arg(arg: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            result.push_str(&"\\".repeat(slashes * 2 + 1));
        } else {
            result.push_str(&"\\".repeat(slashes));
        }
        slashes = 0;
        result.push(ch);
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}
