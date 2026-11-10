//! Node to client, as a client reads it: the frames the node sends, and
//! the JSON its sessions and launcher frames carry.

use serde::Deserialize;

use crate::{
    CLAIM_REFUSED, DELIVERED, DONE, HISTORY, LAUNCHER, MATCH, OUTPUT, Outcome, PANE_GONE, PaneId,
    SCREEN, SEARCHED, SESSIONS, THUMBNAIL,
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Pane {
    pub id: PaneId,
    pub cols: u16,
    pub rows: u16,
    /// The screen asks the operator something: the question, or "" when it
    /// isn't on screen. Absent when the pane isn't waiting.
    #[serde(default)]
    pub waiting: Option<String>,
}

/// Who holds a session's sizing seat, as the node tells one client.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Seat {
    Mine,
    Other,
}

/// Every session the app follows, across nodes, as the terminals page last
/// heard them: the read model it publishes for other pages
/// (`parvi_app_kit::Host::model`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionList {
    pub sessions: Vec<Session>,
}

/// A session as one node lists it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Session {
    /// Which node listed it: the client sets this, the node does not send it.
    #[serde(skip)]
    pub node: String,
    /// tmux's id, like `$3`, or `$3@alice` on another Unix user's server.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub seat: Option<Seat>,
    #[serde(default)]
    pub windows: Option<u32>,
    #[serde(default)]
    pub foreign: Option<u32>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub mouse: Option<bool>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub last: Option<u64>,
    /// What its active pane runs, as tmux names it.
    #[serde(default)]
    pub command: Option<String>,
    /// Its active pane's folder.
    #[serde(default)]
    pub cwd: Option<String>,
    /// What its agent last reported through its hooks.
    #[serde(default)]
    pub event: Option<AgentEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Finished,
    NeedsInput,
}

/// A session's last agent event (docs/protocol.md, Agent events).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AgentEvent {
    pub kind: EventKind,
    /// When the node heard it, Unix ms: with the session, what makes it this
    /// event rather than the one before.
    pub at: u64,
    pub seen: bool,
    #[serde(default)]
    pub summary: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Harness {
    pub id: String,
    /// The program; empty for the shell.
    pub command: String,
    pub flags: String,
}

/// What a node's launcher offers.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Launcher {
    pub harnesses: Vec<Harness>,
    pub folders: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub users: Vec<String>,
    /// Why a launch asked to notify gets no completion hooks on this node.
    #[serde(default, rename = "notifyOff")]
    pub notify_off: Option<String>,
}

/// A frame from the node.
#[derive(Clone, Debug, PartialEq)]
pub enum Down {
    Sessions(Vec<Session>),
    Launcher(Launcher),
    Screen {
        pane: PaneId,
        cols: u16,
        rows: u16,
        bytes: Vec<u8>,
    },
    Output {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    PaneGone(PaneId),
    Delivered {
        id: u32,
        outcome: Outcome,
    },
    Thumbnail {
        pane: PaneId,
        cols: u16,
        rows: u16,
        text: String,
    },
    Done {
        id: u32,
        ok: bool,
        text: String,
    },
    Match {
        id: u32,
        pane: PaneId,
        line: u32,
        context: [String; 3],
    },
    Searched {
        id: u32,
        matches: u32,
        panes: u32,
        truncated: bool,
    },
    History {
        id: u32,
        pane: PaneId,
        text: Option<String>,
    },
    ClaimRefused {
        pane: PaneId,
        reason: String,
    },
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn text(b: &[u8], at: usize) -> String {
    String::from_utf8_lossy(b.get(at..).unwrap_or_default()).into_owned()
}

/// Reads a frame from the node, its kind byte and body apart. Unknown or
/// short frames are `None`.
pub fn decode_down(kind: u8, b: &[u8]) -> Option<Down> {
    Some(match kind {
        SESSIONS => Down::Sessions(serde_json::from_slice(b).ok()?),
        LAUNCHER => Down::Launcher(serde_json::from_slice(b).ok()?),
        SCREEN => Down::Screen {
            pane: u32_at(b, 0)?,
            cols: u16_at(b, 4)?,
            rows: u16_at(b, 6)?,
            bytes: b.get(8..)?.to_vec(),
        },
        OUTPUT => Down::Output {
            pane: u32_at(b, 0)?,
            bytes: b.get(4..)?.to_vec(),
        },
        PANE_GONE => Down::PaneGone(u32_at(b, 0)?),
        DELIVERED => Down::Delivered {
            id: u32_at(b, 4)?,
            outcome: match *b.get(8)? {
                0 => Outcome::Failed,
                1 => Outcome::Delivered,
                _ => Outcome::Unknown,
            },
        },
        THUMBNAIL => Down::Thumbnail {
            pane: u32_at(b, 0)?,
            cols: u16_at(b, 4)?,
            rows: u16_at(b, 6)?,
            text: text(b, 8),
        },
        DONE => Down::Done {
            id: u32_at(b, 0)?,
            ok: *b.get(4)? == 1,
            text: text(b, 5),
        },
        MATCH => {
            let all = text(b, 12);
            let mut lines = all.splitn(3, '\n').map(str::to_string);
            let context = [
                lines.next().unwrap_or_default(),
                lines.next().unwrap_or_default(),
                lines.next().unwrap_or_default(),
            ];
            Down::Match {
                id: u32_at(b, 0)?,
                pane: u32_at(b, 4)?,
                line: u32_at(b, 8)?,
                context,
            }
        }
        SEARCHED => Down::Searched {
            id: u32_at(b, 0)?,
            matches: u32_at(b, 4)?,
            panes: u32_at(b, 8)?,
            truncated: *b.get(12)? == 1,
        },
        HISTORY => Down::History {
            id: u32_at(b, 0)?,
            pane: u32_at(b, 4)?,
            text: (*b.get(8)? == 1).then(|| text(b, 9)),
        },
        CLAIM_REFUSED => Down::ClaimRefused {
            pane: u32_at(b, 0)?,
            reason: text(b, 4),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{INPUT, Up, delivered, encode_up, screen};

    /// A whole frame, as it came off the socket.
    fn decode(f: &[u8]) -> Option<Down> {
        let (&kind, body) = f.split_first()?;
        decode_down(kind, body)
    }

    #[test]
    fn decodes_each_outcome_of_a_send() {
        for outcome in [Outcome::Failed, Outcome::Delivered, Outcome::Unknown] {
            assert_eq!(
                decode(&delivered(3, 9, outcome)),
                Some(Down::Delivered { id: 9, outcome })
            );
        }
        let mut f = delivered(3, 9, Outcome::Unknown);
        f[9] = 7;
        assert_eq!(
            decode(&f),
            Some(Down::Delivered {
                id: 9,
                outcome: Outcome::Unknown
            }),
            "an outcome it does not know is unknown"
        );
    }

    #[test]
    fn decodes_a_screen_and_output() {
        assert_eq!(
            decode(&screen(7, 120, 40, b"hi")),
            Some(Down::Screen {
                pane: 7,
                cols: 120,
                rows: 40,
                bytes: b"hi".to_vec()
            })
        );
        let input = encode_up(&Up::Input(7, b"x".to_vec()));
        assert_eq!(input[0], INPUT);
        assert!(
            decode(&input).is_none(),
            "an up frame kind is not a down frame"
        );
    }

    #[test]
    fn decodes_sessions_with_their_facts() {
        let json = br#"[{"id":"$3@alice","name":"build","user":"alice","panes":[{"id":4,"cols":80,"rows":24,"waiting":""}],"seat":"other","foreign":1,"active":true,"last":5,"event":{"kind":"needs-input","at":9,"seen":false}}]"#;
        let mut f = vec![SESSIONS];
        f.extend_from_slice(json);
        let Some(Down::Sessions(list)) = decode(&f) else {
            panic!("not sessions")
        };
        assert_eq!(list[0].user.as_deref(), Some("alice"));
        assert_eq!(list[0].panes[0].waiting.as_deref(), Some(""));
        assert_eq!(list[0].seat, Some(Seat::Other));
        assert_eq!(list[0].foreign, Some(1));
        let event = list[0].event.as_ref().unwrap();
        assert_eq!(
            (event.kind, event.at, event.summary.as_str()),
            (EventKind::NeedsInput, 9, "")
        );
    }

    #[test]
    fn decodes_a_match_with_three_lines_of_context() {
        let mut f = vec![MATCH];
        for n in [9u32, 4, 120] {
            f.extend_from_slice(&n.to_le_bytes());
        }
        f.extend_from_slice(b"before\nhit\nafter");
        assert_eq!(
            decode(&f),
            Some(Down::Match {
                id: 9,
                pane: 4,
                line: 120,
                context: ["before".into(), "hit".into(), "after".into()]
            })
        );
    }

    #[test]
    fn a_short_frame_is_not_a_frame() {
        assert_eq!(decode(&[DONE, 1, 0]), None);
        assert_eq!(decode(&[]), None);
    }
}
