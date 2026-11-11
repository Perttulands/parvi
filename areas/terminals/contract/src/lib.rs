//! The terminals area's wire: binary frames on the node's socket whose
//! first byte says what they are. Numbers are little-endian. The node and
//! its clients both use these, so a changed frame fails the build on the
//! other side.

use parvi_core::Error;
use parvi_core::frame::Kinds;

mod down;
pub mod session_copy;
pub use down::{
    AgentEvent, Down, EventKind, Harness, Launcher, Pane, Seat, Session, SessionList, decode_down,
};

/// Every frame kind the area owns, both ways. 0x1b (the fleet's nodes) and
/// 0x04, 0x1c, 0x1d and 0x2a (settings) keep their numbers for those areas.
pub const KINDS: Kinds = &[0x01..=0x02, 0x10..=0x1a, 0x1e..=0x29, 0x2b..=0x2b];

/// A harness option reported by its help output.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct Flag {
    pub name: String,
    pub short: Option<String>,
    pub value: Option<String>,
    pub description: String,
    #[serde(default)]
    pub values: Vec<String>,
}

/// A pane, by its node-wide id.
pub type PaneId = u32;

/// A session command that could not do what it was asked; the message
/// says why.
pub const COMMAND_FAILED: &str = "session_failed";

/// A flag catalogue request named an unconfigured harness.
pub const UNKNOWN_HARNESS: &str = "unknown_harness";
/// A configured harness binary could not be found.
pub const HARNESS_NOT_FOUND: &str = "harness_not_found";
/// A harness did not finish printing help in time.
pub const HARNESS_HELP_TIMEOUT: &str = "harness_help_timeout";
/// A harness exited unsuccessfully while printing help.
pub const HARNESS_HELP_FAILED: &str = "harness_help_failed";
/// A harness help process or its output could not be read.
pub const HARNESS_HELP_ERROR: &str = "harness_help_error";

/// A frame that cannot be read.
pub const BAD_FRAME: &str = "bad_frame";

fn bad(message: impl Into<String>) -> Error {
    Error::new(BAD_FRAME, message)
}

/// `bail!` for frames: returns a [`BAD_FRAME`] error.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(bad(format!($($arg)*)))
    };
}

/// Client to node.
pub const SUBSCRIBE: u8 = 0x10;
pub const UNSUBSCRIBE: u8 = 0x11;
pub const INPUT: u8 = 0x12;
pub const CLAIM: u8 = 0x13;
pub const RELEASE: u8 = 0x14;
pub const SEND: u8 = 0x15;
pub const OVERVIEW: u8 = 0x16;
pub const LAUNCH: u8 = 0x17;
pub const DELETE: u8 = 0x18;
pub const SEARCH: u8 = 0x19;
pub const HISTORY_OF: u8 = 0x1a;
pub const SEEN: u8 = 0x1e;
pub const RENAME: u8 = 0x1f;
/// Up, though past the node-to-client block's start: 0x10-0x1f is full.
pub const MOUSE: u8 = 0x2b;

/// Node to client.
pub const SESSIONS: u8 = 0x01;
pub const LAUNCHER: u8 = 0x02;
pub const SCREEN: u8 = 0x20;
pub const OUTPUT: u8 = 0x21;
pub const PANE_GONE: u8 = 0x22;
pub const DELIVERED: u8 = 0x23;
pub const THUMBNAIL: u8 = 0x24;
pub const DONE: u8 = 0x25;
pub const MATCH: u8 = 0x26;
pub const SEARCHED: u8 = 0x27;
pub const HISTORY: u8 = 0x28;
pub const CLAIM_REFUSED: u8 = 0x29;

/// The most text one send frame may deliver.
pub const MAX_SEND: usize = 64 * 1024;

/// A frame from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Up {
    Subscribe(PaneId),
    Unsubscribe(PaneId),
    Input(PaneId, Vec<u8>),
    /// Take the sizing seat for the pane's session at this grid (cols, rows).
    Claim(PaneId, u16, u16),
    Release(PaneId),
    /// Deliver text into the pane's prompt as a paste, then press Enter if
    /// asked. `id` comes back in the delivered frame. `ready`: the pane was
    /// just launched, so wait until it is ready for a paste (#242).
    Send {
        pane: PaneId,
        id: u32,
        enter: bool,
        ready: bool,
        text: String,
    },
    /// Start (true) or stop the overview's thumbnails. No pane.
    Overview(bool),
    /// Create a session and start a harness in it. No pane; `id` comes back
    /// in the done frame.
    Launch {
        id: u32,
        launch: Launch,
    },
    /// Kill exactly this session, by its id like `$3`. No pane.
    Delete {
        id: u32,
        session: String,
    },
    /// Rename exactly this session, by its id, to `name`. No pane.
    Rename {
        id: u32,
        session: String,
        name: String,
    },
    /// Turn tmux's mouse mode on (true) or off on every tmux server the
    /// node follows. No pane; `id` comes back in the done frame.
    Mouse {
        id: u32,
        on: bool,
    },
    /// Search every pane's scrollback; an empty query stops the search
    /// running. No pane; matches and the end carry `id`.
    Search {
        id: u32,
        query: String,
    },
    /// The pane's whole history and screen, to show read-only.
    History {
        pane: PaneId,
        id: u32,
    },
    /// The operator looked at this session, by its hub id, when its agent
    /// event was the one heard at `event` (its `at`): that event is no
    /// longer news. No pane.
    Seen {
        session: String,
        event: u64,
    },
}

/// A launch's `<cols>x<rows>`: the grid of the window it goes into.
fn launch_size(text: &str) -> Result<(u16, u16), Error> {
    let grid = text
        .split_once('x')
        .and_then(|(cols, rows)| Some((cols.parse().ok()?, rows.parse().ok()?)));
    match grid {
        Some((cols, rows)) if cols > 0 && rows > 0 => Ok((cols, rows)),
        _ => Err(bad(format!("launch size is <cols>x<rows>, not {text:?}"))),
    }
}

pub fn decode_up(frame: &[u8]) -> Result<Up, Error> {
    let Some((&kind, rest)) = frame.split_first() else {
        bail!("empty frame");
    };
    match kind {
        OVERVIEW => {
            let [on] = rest else {
                bail!("frame 0x{kind:02x} needs one byte");
            };
            return Ok(Up::Overview(*on != 0));
        }
        MOUSE => {
            let [a, b, c, d, on] = rest else {
                bail!("mouse frame needs an id and one byte");
            };
            return Ok(Up::Mouse {
                id: u32::from_le_bytes([*a, *b, *c, *d]),
                on: *on != 0,
            });
        }
        SEEN => {
            if rest.len() < 8 {
                bail!("seen frame has no event");
            }
            return Ok(Up::Seen {
                event: u64::from_le_bytes(rest[..8].try_into().unwrap()),
                session: String::from_utf8(rest[8..].to_vec())
                    .map_err(|_| bad("session is not UTF-8"))?,
            });
        }
        SEARCH => {
            if rest.len() < 4 {
                bail!("search frame has no id");
            }
            return Ok(Up::Search {
                id: u32::from_le_bytes(rest[..4].try_into().unwrap()),
                query: String::from_utf8(rest[4..].to_vec())
                    .map_err(|_| bad("query is not UTF-8"))?,
            });
        }
        LAUNCH | DELETE | RENAME => {
            if rest.len() < 4 {
                bail!("frame 0x{kind:02x} has no id");
            }
            let id = u32::from_le_bytes(rest[..4].try_into().unwrap());
            let text =
                std::str::from_utf8(&rest[4..]).map_err(|_| bad("frame text is not UTF-8"))?;
            if kind == DELETE {
                return Ok(Up::Delete {
                    id,
                    session: text.to_owned(),
                });
            }
            if kind == RENAME {
                let Some((session, name)) = text.split_once('\0') else {
                    bail!("rename frame needs a session and a name");
                };
                return Ok(Up::Rename {
                    id,
                    session: session.to_owned(),
                    name: name.to_owned(),
                });
            }
            let fields: Vec<&str> = text.split('\0').collect();
            // A fifth field, the user, is optional: none means the service
            // user's own tmux server. A sixth, "1", asks for completion hooks.
            // A seventh, `<cols>x<rows>`, is the grid of the window the
            // session is launched into.
            let (&[harness, folder, name, flags], user, notify, size) = match fields[..] {
                [a, b, c, d] => (&[a, b, c, d], "", "", None),
                [a, b, c, d, user] => (&[a, b, c, d], user, "", None),
                [a, b, c, d, user, notify] => (&[a, b, c, d], user, notify, None),
                [a, b, c, d, user, notify, size] => {
                    (&[a, b, c, d], user, notify, Some(launch_size(size)?))
                }
                _ => bail!("launch frame needs four to seven fields"),
            };
            return Ok(Up::Launch {
                id,
                launch: Launch {
                    harness: harness.into(),
                    folder: folder.into(),
                    name: name.into(),
                    flags: flags.into(),
                    user: user.into(),
                    notify: notify == "1",
                    size,
                },
            });
        }
        _ => {}
    }
    if rest.len() < 4 {
        bail!("frame 0x{kind:02x} has no pane id");
    }
    let pane = u32::from_le_bytes(rest[..4].try_into().unwrap());
    let body = &rest[4..];
    Ok(match kind {
        SUBSCRIBE => Up::Subscribe(pane),
        UNSUBSCRIBE => Up::Unsubscribe(pane),
        INPUT => Up::Input(pane, body.to_vec()),
        CLAIM if body.len() == 4 => Up::Claim(
            pane,
            u16::from_le_bytes([body[0], body[1]]),
            u16::from_le_bytes([body[2], body[3]]),
        ),
        CLAIM => bail!("claim frame needs cols and rows"),
        RELEASE => Up::Release(pane),
        HISTORY_OF if body.len() == 4 => Up::History {
            pane,
            id: u32::from_le_bytes(body.try_into().unwrap()),
        },
        HISTORY_OF => bail!("history frame needs an id"),
        SEND => {
            if body.len() < 5 {
                bail!("send frame needs an id and flags");
            }
            let text = &body[5..];
            if text.len() > MAX_SEND {
                bail!("send frame carries more than {MAX_SEND} bytes");
            }
            Up::Send {
                pane,
                id: u32::from_le_bytes(body[..4].try_into().unwrap()),
                enter: body[4] & 1 != 0,
                ready: body[4] & 2 != 0,
                text: String::from_utf8(text.to_vec())
                    .map_err(|_| bad("send text is not UTF-8"))?,
            }
        }
        _ => bail!("unknown frame type 0x{kind:02x}"),
    })
}

pub fn encode_up(up: &Up) -> Vec<u8> {
    let body_bytes;
    let (kind, pane, body): (u8, PaneId, &[u8]) = match up {
        Up::Subscribe(pane) => (SUBSCRIBE, *pane, &[]),
        Up::Unsubscribe(pane) => (UNSUBSCRIBE, *pane, &[]),
        Up::Input(pane, data) => (INPUT, *pane, data),
        Up::Claim(pane, cols, rows) => {
            body_bytes = [cols.to_le_bytes(), rows.to_le_bytes()].concat();
            (CLAIM, *pane, &body_bytes)
        }
        Up::Release(pane) => (RELEASE, *pane, &[]),
        Up::Send {
            pane,
            id,
            enter,
            ready,
            text,
        } => {
            let flags = u8::from(*enter) | u8::from(*ready) << 1;
            body_bytes = [&id.to_le_bytes()[..], &[flags], text.as_bytes()].concat();
            (SEND, *pane, &body_bytes)
        }
        Up::Overview(on) => return vec![OVERVIEW, u8::from(*on)],
        Up::Launch { id, launch } => {
            let mut text = [&launch.harness, &launch.folder, &launch.name, &launch.flags]
                .map(String::as_str)
                .join("\0");
            if !launch.user.is_empty() || launch.notify || launch.size.is_some() {
                text.push('\0');
                text.push_str(&launch.user);
            }
            if launch.notify || launch.size.is_some() {
                text.push('\0');
                text.push_str(if launch.notify { "1" } else { "" });
            }
            if let Some((cols, rows)) = launch.size {
                text.push_str(&format!("\0{cols}x{rows}"));
            }
            return [&[LAUNCH][..], &id.to_le_bytes(), text.as_bytes()].concat();
        }
        Up::Delete { id, session } => {
            return [&[DELETE][..], &id.to_le_bytes(), session.as_bytes()].concat();
        }
        Up::Rename { id, session, name } => {
            let text = format!("{session}\0{name}");
            return [&[RENAME][..], &id.to_le_bytes(), text.as_bytes()].concat();
        }
        Up::Mouse { id, on } => {
            return [&[MOUSE][..], &id.to_le_bytes(), &[u8::from(*on)]].concat();
        }
        Up::Seen { session, event } => {
            return [&[SEEN][..], &event.to_le_bytes(), session.as_bytes()].concat();
        }
        Up::Search { id, query } => {
            return [&[SEARCH][..], &id.to_le_bytes(), query.as_bytes()].concat();
        }
        Up::History { pane, id } => {
            body_bytes = id.to_le_bytes().to_vec();
            (HISTORY_OF, *pane, &body_bytes[..])
        }
    };
    let mut frame = Vec::with_capacity(5 + body.len());
    frame.push(kind);
    frame.extend_from_slice(&pane.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

/// One launch, as a client asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub harness: String,
    pub folder: String,
    pub name: String,
    /// The whole flag line, as edited; empty for none.
    pub flags: String,
    /// Whose tmux server it starts on; empty for the service user's own.
    pub user: String,
    /// Install the harness's completion hooks, if it takes them
    /// (the node's agent events).
    pub notify: bool,
    /// The grid of the window it is launched into, which its window is
    /// given at creation instead of the launcher's size (#161). `None` for
    /// a launch into no window.
    pub size: Option<(u16, u16)>,
}

/// A sizing claim was refused, with the pane id and an explanation.
pub fn claim_refused(pane: PaneId, reason: &str) -> Vec<u8> {
    [&[CLAIM_REFUSED][..], &pane.to_le_bytes(), reason.as_bytes()].concat()
}

/// `0x25`, id u32, ok u8, then UTF-8 text: the answer to launch or delete
/// frame `id`. A launch that worked carries the new session's id; anything
/// that failed carries why.
pub fn done(id: u32, ok: bool, text: &str) -> Vec<u8> {
    [
        &[DONE][..],
        &id.to_le_bytes(),
        &[u8::from(ok)],
        text.as_bytes(),
    ]
    .concat()
}

/// `0x28`, id u32, pane u32, ok u8, then the pane's history and screen with
/// colour escapes, one line per row, rows separated by LF.
pub fn history(id: u32, pane: PaneId, capture: Option<&str>) -> Vec<u8> {
    [
        &[HISTORY][..],
        &id.to_le_bytes(),
        &pane.to_le_bytes(),
        &[u8::from(capture.is_some())],
        capture.unwrap_or("").as_bytes(),
    ]
    .concat()
}

/// `0x20`, pane, cols u16, rows u16, then bytes to draw on a reset terminal.
pub fn screen(pane: PaneId, cols: u16, rows: u16, bytes: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(9 + bytes.len());
    frame.push(SCREEN);
    frame.extend_from_slice(&pane.to_le_bytes());
    frame.extend_from_slice(&cols.to_le_bytes());
    frame.extend_from_slice(&rows.to_le_bytes());
    frame.extend_from_slice(bytes);
    frame
}

/// `0x21`, pane, then the bytes the pane wrote.
pub fn output(pane: PaneId, bytes: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + bytes.len());
    frame.push(OUTPUT);
    frame.extend_from_slice(&pane.to_le_bytes());
    frame.extend_from_slice(bytes);
    frame
}

/// What became of a send, as the delivered frame says it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing went in (0).
    Failed = 0,
    /// tmux took the text and its Enter (1). Delivered, not read.
    Delivered = 1,
    /// The text went in and its Enter did not, or a just-launched pane
    /// never got ready for it (2): look before sending again.
    Unknown = 2,
}

/// `0x23`, pane, id u32, outcome u8: what became of send frame `id`.
pub fn delivered(pane: PaneId, id: u32, outcome: Outcome) -> Vec<u8> {
    let mut frame = vec![DELIVERED];
    frame.extend_from_slice(&pane.to_le_bytes());
    frame.extend_from_slice(&id.to_le_bytes());
    frame.push(outcome as u8);
    frame
}

/// `0x24`, pane, cols u16, rows u16, then the pane's visible rows as UTF-8
/// text, one line per row.
pub fn thumbnail(pane: PaneId, cols: u16, rows: u16, text: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(9 + text.len());
    frame.push(THUMBNAIL);
    frame.extend_from_slice(&pane.to_le_bytes());
    frame.extend_from_slice(&cols.to_le_bytes());
    frame.extend_from_slice(&rows.to_le_bytes());
    frame.extend_from_slice(text.as_bytes());
    frame
}

/// `0x22`, pane: the pane no longer exists.
pub fn pane_gone(pane: PaneId) -> Vec<u8> {
    let mut frame = vec![PANE_GONE];
    frame.extend_from_slice(&pane.to_le_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_owns_every_kind_it_sends_and_takes() {
        for kind in [
            SUBSCRIBE,
            UNSUBSCRIBE,
            INPUT,
            CLAIM,
            RELEASE,
            SEND,
            OVERVIEW,
            LAUNCH,
            DELETE,
            SEARCH,
            HISTORY_OF,
            SEEN,
            RENAME,
            MOUSE,
            SESSIONS,
            LAUNCHER,
            SCREEN,
            OUTPUT,
            PANE_GONE,
            DELIVERED,
            THUMBNAIL,
            DONE,
            MATCH,
            SEARCHED,
            HISTORY,
            CLAIM_REFUSED,
        ] {
            assert!(parvi_core::frame::owns(KINDS, kind), "{kind:#04x}");
        }
        for kind in [0x00, 0x04, 0x1b, 0x1c, 0x1d, 0x2a, 0x30] {
            assert!(!parvi_core::frame::owns(KINDS, kind), "{kind:#04x}");
        }
    }

    #[test]
    fn up_frames_round_trip() {
        for up in [
            Up::Subscribe(7),
            Up::Unsubscribe(70000),
            Up::Input(3, b"ls\r".to_vec()),
            Up::Input(3, vec![]),
            Up::Claim(4, 200, 50),
            Up::Release(4),
            Up::Send {
                pane: 5,
                id: 9,
                enter: true,
                ready: false,
                text: "two\nlines é".into(),
            },
            Up::Send {
                pane: 5,
                id: 10,
                enter: false,
                ready: true,
                text: "into a new agent".into(),
            },
            Up::Overview(true),
            Up::Overview(false),
            Up::Launch {
                id: 3,
                launch: Launch {
                    harness: "claude-code".into(),
                    folder: "~/work".into(),
                    name: "claude-work".into(),
                    flags: "--model x".into(),
                    user: String::new(),
                    notify: false,
                    size: None,
                },
            },
            Up::Launch {
                id: 5,
                launch: Launch {
                    harness: "shell".into(),
                    folder: "~".into(),
                    name: "fitted".into(),
                    flags: String::new(),
                    user: String::new(),
                    notify: false,
                    size: Some((97, 31)),
                },
            },
            Up::Launch {
                id: 4,
                launch: Launch {
                    harness: "codex".into(),
                    folder: "~".into(),
                    name: "told".into(),
                    flags: String::new(),
                    user: String::new(),
                    notify: true,
                    size: None,
                },
            },
            Up::Launch {
                id: 8,
                launch: Launch {
                    harness: "shell".into(),
                    folder: "~".into(),
                    name: "theirs".into(),
                    flags: String::new(),
                    user: "alice".into(),
                    notify: true,
                    size: Some((300, 80)),
                },
            },
            Up::Delete {
                id: 4,
                session: "$7".into(),
            },
            Up::Rename {
                id: 6,
                session: "$3@alice".into(),
                name: "renamed-2".into(),
            },
            Up::Mouse { id: 7, on: true },
            Up::Mouse { id: 8, on: false },
            Up::Search {
                id: 5,
                query: "erreur é".into(),
            },
            Up::History { pane: 6, id: 7 },
            Up::Seen {
                session: "$3@alice".into(),
                event: 1_762_370_000_123,
            },
        ] {
            assert_eq!(decode_up(&encode_up(&up)).unwrap(), up);
        }
    }

    /// The app's encoders, carried as bytes: what a client puts on the wire.
    #[test]
    fn encodes_send_launch_and_rename() {
        let send = |id, enter, ready| {
            encode_up(&Up::Send {
                pane: 3,
                id,
                enter,
                ready,
                text: "go".into(),
            })
        };
        let f = send(11, true, false);
        assert_eq!(f[0], SEND);
        assert_eq!(&f[1..5], &3u32.to_le_bytes());
        assert_eq!(&f[5..9], &11u32.to_le_bytes());
        assert_eq!(f[9], 1);
        assert_eq!(&f[10..], b"go");
        assert_eq!(send(12, false, true)[9], 2);
        let launch =
            |harness: &str, folder: &str, name: &str, flags: &str, user: &str, notify, size| {
                encode_up(&Up::Launch {
                    id: 2,
                    launch: Launch {
                        harness: harness.into(),
                        folder: folder.into(),
                        name: name.into(),
                        flags: flags.into(),
                        user: user.into(),
                        notify,
                        size,
                    },
                })
            };
        assert_eq!(
            &launch("claude", "~", "c", "", "", true, None)[5..],
            b"claude\0~\0c\0\0\x001"
        );
        assert_eq!(
            &launch("shell", "/", "s", "-x", "bob", false, None)[5..],
            b"shell\0/\0s\0-x\0bob"
        );
        assert_eq!(
            &launch("shell", "/", "s", "", "", false, Some((97, 31)))[5..],
            b"shell\0/\0s\0\0\0\x0097x31"
        );
        let r = encode_up(&Up::Rename {
            id: 4,
            session: "$3@bob".into(),
            name: "omega".into(),
        });
        assert_eq!((r[0], &r[1..5]), (RENAME, &4u32.to_le_bytes()[..]));
        assert_eq!(&r[5..], b"$3@bob\0omega");
        assert_eq!(
            encode_up(&Up::Mouse { id: 5, on: true }),
            [MOUSE, 5, 0, 0, 0, 1]
        );
        assert_eq!(
            encode_up(&Up::Mouse { id: 6, on: false }),
            [MOUSE, 6, 0, 0, 0, 0]
        );
    }

    #[test]
    fn rejects_short_and_unknown_frames() {
        assert!(decode_up(&[]).is_err());
        assert!(decode_up(&[SUBSCRIBE, 1, 0]).is_err());
        assert!(decode_up(&[0x7f, 1, 0, 0, 0]).is_err());
        assert!(decode_up(&[CLAIM, 1, 0, 0, 0, 80, 0]).is_err());
        assert!(decode_up(&[SEND, 1, 0, 0, 0, 9, 0, 0, 0]).is_err());
        assert!(decode_up(&[SEND, 1, 0, 0, 0, 9, 0, 0, 0, 0, 0xff]).is_err());
        assert!(decode_up(&[OVERVIEW]).is_err());
        assert!(decode_up(&[OVERVIEW, 1, 0]).is_err());
        assert!(decode_up(&[LAUNCH, 1, 0, 0]).is_err());
        assert!(decode_up(&[RENAME, 1, 0, 0, 0, b'$', b'3']).is_err());
        assert!(decode_up(&[MOUSE, 1, 0, 0, 0]).is_err());
        assert!(decode_up(&[MOUSE, 1, 0, 0, 0, 1, 1]).is_err());
        assert!(decode_up(&[LAUNCH, 1, 0, 0, 0, b'a', 0, b'b']).is_err());
        for size in ["", "80", "0x24", "80x0", "80x24x1", "-1x24"] {
            let text = format!("shell\0~\0s\0\0\0\0{size}");
            let frame = [&[LAUNCH, 1, 0, 0, 0][..], text.as_bytes()].concat();
            assert!(decode_up(&frame).is_err(), "{size:?}");
        }
    }

    #[test]
    fn encodes_screen_and_output() {
        assert_eq!(
            screen(0x0102_0304, 200, 50, b"x"),
            vec![SCREEN, 4, 3, 2, 1, 200, 0, 50, 0, b'x']
        );
        assert_eq!(output(1, b"ab"), vec![OUTPUT, 1, 0, 0, 0, b'a', b'b']);
        assert_eq!(pane_gone(2), vec![PANE_GONE, 2, 0, 0, 0]);
        assert_eq!(
            thumbnail(3, 80, 2, "a\n"),
            vec![THUMBNAIL, 3, 0, 0, 0, 80, 0, 2, 0, b'a', b'\n']
        );
        assert_eq!(done(5, false, "no"), vec![DONE, 5, 0, 0, 0, 0, b'n', b'o']);
        assert_eq!(
            delivered(2, 7, Outcome::Delivered),
            vec![DELIVERED, 2, 0, 0, 0, 7, 0, 0, 0, 1]
        );
        assert_eq!(
            delivered(2, 7, Outcome::Unknown),
            vec![DELIVERED, 2, 0, 0, 0, 7, 0, 0, 0, 2]
        );
    }
}
