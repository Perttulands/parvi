//! Telling the operator what an agent reported (docs/protocol.md, Agent
//! events), as CHROTE tells it: the status line always, the toast, a tone, a
//! mark on the session's row and tab until he looks at it, and a desktop
//! notification while the window is not the active one. Looking is the
//! focused window of the terminal page showing the session while this window
//! is active, or right after the operator acted on the page or showed the
//! session (which needs no window manager to tell); it tells the node, so
//! every device agrees.

use super::page::Terminals;
use crate::store::sessions::{self, Key};
use gpui::{Context, SystemNotification, Window};
use parvi_terminals_contract::{AgentEvent, Session};

impl Terminals {
    /// The sessions whose agent just reported.
    pub fn reported(&mut self, list: Vec<Session>, window: &mut Window, cx: &mut Context<Self>) {
        for s in &list {
            let Some(e) = &s.event else { continue };
            let title = sessions::report_title(s, e);
            self.report(&title, false, None);
            if self.settings.event_toast {
                self.toast(&title, cx);
            }
            if self.settings.event_tone {
                crate::sound::play(e.kind);
            }
            if self.settings.event_desktop && !window.is_window_active() {
                let tag = Key::of(s).text();
                let page = self.me.clone();
                let clicked = tag.clone();
                self.host.notify(
                    SystemNotification {
                        tag: tag.into(),
                        title: title.into(),
                        body: e.summary.clone().into(),
                        actions: vec![],
                    },
                    move |window, cx| {
                        let _ =
                            page.update(cx, |p, cx| p.notification_clicked(&clicked, window, cx));
                    },
                    cx,
                );
            }
        }
    }

    /// The event a session's row and tab are marked with, while it is news
    /// and marks are on.
    pub fn marked<'a>(&self, s: &'a Session) -> Option<&'a AgentEvent> {
        sessions::news(s).filter(|_| self.settings.event_marks)
    }

    /// Whether a session bound in a window a tab shows is marked: it can
    /// be found in that tab.
    pub fn tab_marked(&self, tab: usize) -> bool {
        self.settings.workspace.tabs.get(tab).is_some_and(|tb| {
            tb.shown().iter().flat_map(|w| &w.sessions).any(|b| {
                self.fleet
                    .session(&b.key)
                    .is_some_and(|s| self.marked(s).is_some())
            })
        })
    }

    /// Tells the node the operator is looking at the focused session, once
    /// per report. Runs as the window draws: a lookup and a compare.
    pub fn see_focused(&mut self, window: &Window) {
        let acted = std::mem::take(&mut self.acted);
        if !self.shown || !(acted || window.is_window_active()) {
            return;
        }
        let Some(key) = self.focused_key() else {
            return;
        };
        let Some(s) = self.fleet.session(&key) else {
            return;
        };
        let Some(e) = sessions::news(s) else {
            return;
        };
        let at = e.at;
        let told = self.seen_sent.insert(key.text(), at);
        if told != Some(at) {
            let s = s.clone();
            self.fleet.seen(&s, at);
        }
    }

    /// A desktop notification was clicked: show its session here.
    fn notification_clicked(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.fleet.all.iter().find(|s| Key::of(s).text() == tag) else {
            return;
        };
        self.place(Key::of(s), window, cx);
    }
}
