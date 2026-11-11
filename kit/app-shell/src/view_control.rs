//! View control (docs/protocol.md, View): the serving node is told which
//! page the window shows, when its link opens and after each change, and
//! an agent's `open` command arrives from it and is answered. The command
//! names a page by its id and at most one thing to show on it, which goes
//! to the page as an ask to reveal it.

use crate::shell::Shell;
use gpui::{Context, Task, Window};
use parvi_app_kit::Ask;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::link::socket::LinkId;
use parvi_core::NodeId;
use serde::Serialize;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};

/// What the app shows, up.
const VIEW: u8 = 0xb0;
/// The answer to a view command, up.
const VIEW_DONE: u8 = 0xb1;
/// A view command, down.
pub const VIEW_DO: u8 = 0xb8;

/// The serving node hears of changes at most this often.
const EVERY: Duration = Duration::from_millis(250);

/// What the app shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct View {
    pub page: Option<String>,
}

pub fn view(json: &[u8]) -> Vec<u8> {
    [&[VIEW][..], json].concat()
}

/// The answer to view command `id`.
pub fn view_done(id: u32, json: &[u8]) -> Vec<u8> {
    [&[VIEW_DONE][..], &id.to_le_bytes(), json].concat()
}

/// A view command's body: its id, then its JSON. `None` if too short.
pub fn view_do(body: &[u8]) -> Option<(u32, &[u8])> {
    let id = u32::from_le_bytes(body.get(..4)?.try_into().ok()?);
    Some((id, &body[4..]))
}

/// An `open` command: the page, and what to reveal on it.
#[derive(Debug, PartialEq, Eq)]
pub struct Open {
    pub page: String,
    pub reveal: Option<Ask>,
}

/// Reads a view command. `open` takes `page`, then at most one thing to
/// show (`path`, `item`, `run`, …) on `node`, at `step` of it.
pub fn command(json: &[u8]) -> Result<Open, String> {
    let mut v: Map<String, Value> =
        serde_json::from_slice(json).map_err(|e| format!("not a view command: {e}"))?;
    let text = |v: &mut Map<String, Value>, k: &str| match v.remove(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("{k} is not text")),
    };
    let what = text(&mut v, "do")?.unwrap_or_default();
    if what != "open" {
        return Err(format!("this app does not do \"{what}\""));
    }
    let page = text(&mut v, "page")?.ok_or("open names a page")?;
    let node = text(&mut v, "node")?;
    let part = text(&mut v, "step")?;
    let mut things = v.into_iter();
    let reveal = match (things.next(), things.next()) {
        (None, _) => None,
        (Some((what, Value::String(id))), None) => Some(Ask::Reveal {
            what,
            id,
            node: node.map(NodeId),
            part,
            beside: false,
        }),
        (Some((what, _)), None) => return Err(format!("{what} is not text")),
        (Some(_), Some(_)) => return Err("open shows one thing at a time".into()),
    };
    Ok(Open { page, reveal })
}

#[derive(Default)]
pub struct ViewControl {
    /// What the serving node was last told, on which link.
    told: Option<(LinkId, View)>,
    at: Option<Instant>,
    /// A change waiting out the time since the last report.
    waiting: Option<Task<()>>,
}

impl Shell {
    fn view_snapshot(&self) -> View {
        View {
            page: self.shown_page().map(str::to_string),
        }
    }

    /// Each draw: the serving node hears the view if it changed since it
    /// last did on this link, at most every 250 ms. Nothing waits while
    /// nothing changed.
    pub(crate) fn view_changed(&mut self, cx: &mut Context<Self>) {
        let here = self.hub.read(cx).fleet.here();
        let link = here.link_id();
        if self.view.waiting.is_some()
            || here.state != LinkState::Open
            || self.view.told.as_ref() == Some(&(link, self.view_snapshot()))
        {
            return;
        }
        let new_link = self.view.told.as_ref().is_none_or(|(l, _)| *l != link);
        let wait = match self.view.at {
            Some(t) if !new_link => EVERY.saturating_sub(t.elapsed()),
            _ => Duration::ZERO,
        };
        if wait.is_zero() {
            self.view_tell(cx);
            return;
        }
        self.view.waiting = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |s, cx| {
                s.view.waiting = None;
                s.view_changed(cx);
            });
        }));
    }

    fn view_tell(&mut self, cx: &mut Context<Self>) {
        let view = self.view_snapshot();
        let json = serde_json::to_vec(&view).expect("a view is JSON");
        let here = self.hub.read(cx).fleet.here();
        if here.send(self::view(&json)) {
            self.view.told = Some((here.link_id(), view));
            self.view.at = Some(Instant::now());
        }
    }

    /// A view command from `node`: applied and answered on the serving
    /// node's link, the only one that takes them.
    pub(crate) fn view_do(
        &mut self,
        node: &str,
        body: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Some(node) != self.hub.read(cx).fleet.here_id() {
            log::warn!("a view command from {node}, not the serving node, was ignored");
            return;
        }
        let Some((id, json)) = view_do(body) else {
            log::warn!("a view command frame too short to answer was ignored");
            return;
        };
        let answer = match command(json).and_then(|open| self.view_open(open, window, cx)) {
            Ok(view) => serde_json::json!({ "ok": true, "view": view }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        };
        self.hub
            .read(cx)
            .fleet
            .here()
            .send(view_done(id, answer.to_string().as_bytes()));
    }

    fn view_open(
        &mut self,
        open: Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<View, String> {
        let i = self
            .pages
            .iter()
            .position(|p| p.meta.id == open.page)
            .ok_or_else(|| format!("this app has no page {}", open.page))?;
        if let Some(ask) = &open.reveal {
            let meta = &self.pages[i].meta;
            if !meta.answers.iter().any(|a| ask.answered_by(a)) {
                return Err(format!("{} cannot {}", meta.id, ask.describe()));
            }
        }
        self.show(i, window, cx);
        if let Some(ask) = open.reveal {
            self.pages[i].answer(ask, "", window, cx);
        }
        Ok(self.view_snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_frames_carry_an_id_and_json() {
        assert_eq!(view(b"{}"), b"\xb0{}");
        let done = view_done(7, br#"{"ok":true}"#);
        assert_eq!(done[0], 0xb1);
        assert_eq!(&done[1..5], &7u32.to_le_bytes());
        assert_eq!(&done[5..], br#"{"ok":true}"#);
        let mut down = 9u32.to_le_bytes().to_vec();
        down.extend_from_slice(br#"{"do":"close","tab":1}"#);
        assert_eq!(view_do(&down), Some((9, &br#"{"do":"close","tab":1}"#[..])));
        assert_eq!(view_do(&[1, 0, 0]), None);
    }

    #[test]
    fn open_names_a_page_and_one_thing_to_reveal_on_it() {
        assert_eq!(
            command(br#"{"do":"open","page":"p"}"#),
            Ok(Open {
                page: "p".into(),
                reveal: None
            })
        );
        assert_eq!(
            command(br#"{"do":"open","page":"p","run":"r1","step":"build","node":"n"}"#),
            Ok(Open {
                page: "p".into(),
                reveal: Some(Ask::Reveal {
                    what: "run".into(),
                    id: "r1".into(),
                    node: Some("n".into()),
                    part: Some("build".into()),
                    beside: false,
                })
            })
        );
        assert!(command(br#"{"do":"open","page":"p","path":"/a","item":"x"}"#).is_err());
        assert!(command(br#"{"do":"open"}"#).is_err());
        assert_eq!(
            command(br#"{"do":"close","tab":1}"#),
            Err("this app does not do \"close\"".into())
        );
    }
}
