//! Send to Session, the one hand-off (CHROTE's `SendDrawer`, S22–S32): what
//! the drawer hands over, what the operator is told became of it, and how
//! wide it stands. A surface opens the drawer with a reference, the one line
//! naming the object it was looking at; the reference is shown, not edited,
//! because an edited reference names nothing. The note under it is the
//! operator's. Files attached land on the target's node first, and their
//! paths follow the note.

use crate::store::sessions::Key;
use crate::theme::size;
use parvi_terminals_contract::{Outcome, Session};

/// The message as the session receives it: the reference, the note, then
/// the attached files' paths there, as CHROTE composes it.
pub fn message(reference: Option<&str>, note: &str, files: &[String]) -> String {
    let mut parts = vec![];
    if let Some(line) = reference.map(str::trim).filter(|l| !l.is_empty()) {
        parts.push(line.to_string());
    }
    let note = note.trim_end_matches('\n');
    if !note.is_empty() {
        parts.push(note.to_string());
    }
    if !files.is_empty() {
        let list: Vec<String> = files.iter().map(|p| format!("- {p}")).collect();
        parts.push(format!("Files:\n{}", list.join("\n")));
    }
    parts.join("\n\n")
}

/// What the drawer says when a send to `name` did not simply arrive. An
/// unknown outcome is not sent again from the drawer: the pane may hold it.
pub fn said(outcome: Outcome, name: &str) -> Option<String> {
    match outcome {
        Outcome::Delivered => None,
        Outcome::Failed => Some(format!(
            "Not delivered: the pane is gone or tmux refused it. {name} is unchanged."
        )),
        Outcome::Unknown => Some(format!(
            "Outcome unknown: the text may be in {name} without its Enter, a new \
             session never got ready for it, or the connection dropped first. \
             Look at the pane before sending again."
        )),
    }
}

/// What the operator is told once a send to `name` arrived: a launch's
/// message waits on the new agent's line, unsubmitted; a send without
/// Enter was pasted; one with it was sent (#242).
pub fn arrived(name: &str, launched: bool, enter: bool) -> String {
    if launched {
        format!("Launched {name}; your message waits on its line. Press Enter there to send it.")
    } else if enter {
        format!("Sent to {name}")
    } else {
        format!("Pasted into {name}")
    }
}

/// What became of the drawer's launch, as a launch answer reads.
#[derive(Debug, PartialEq)]
pub enum Landed {
    /// Some other launcher's launch: not the drawer's business.
    NotOurs,
    /// The launch failed; its launcher says why.
    Failed,
    /// It launched as this session id: paste once it is listed with a pane.
    Wait(String),
}

/// The drawer awaits `awaited` (node, request); `node` answered request
/// `id` with `ok` and `session`, the new session's id (then a NUL and any
/// note).
pub fn landed(
    awaited: Option<&(String, u32)>,
    node: &str,
    id: u32,
    ok: bool,
    session: &str,
) -> Landed {
    if awaited != Some(&(node.to_string(), id)) {
        return Landed::NotOurs;
    }
    if !ok {
        return Landed::Failed;
    }
    Landed::Wait(session.split('\0').next().unwrap_or_default().to_string())
}

/// Where and how the launched session `sid` on `node` gets its message,
/// once `all` lists it with a pane (a launch answer can come before its
/// panes are). Never with Enter: a new agent may still be starting and
/// would lose a submit, so the message waits on its line.
pub fn launch_paste(all: &[Session], node: &str, sid: &str) -> Option<(Key, bool)> {
    all.iter()
        .find(|s| s.node == node && s.id == sid && !s.panes.is_empty())
        .map(|s| (Key::of(s), false))
}

/// The drawer's width: the one remembered on this device, else the
/// default; never under the least, and never so wide that the workspace
/// beside it keeps less than its share of `room`.
pub fn width(remembered: Option<f32>, room: f32) -> f32 {
    let wanted = remembered
        .filter(|w| w.is_finite())
        .unwrap_or(size::DRAWER)
        .max(size::DRAWER_MIN);
    wanted.min((room - size::DRAWER_ROOM).max(size::DRAWER_MIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_comes_first_then_the_note_then_the_files() {
        assert_eq!(
            message(Some("path /srv/x.rs"), "look at this\n", &[]),
            "path /srv/x.rs\n\nlook at this"
        );
        assert_eq!(message(Some("library a/b"), "", &[]), "library a/b");
        assert_eq!(message(None, "just a note", &[]), "just a note");
        assert_eq!(
            message(
                Some("  "),
                "see",
                &["/d/1/a.png".into(), "/d/2/b.txt".into()]
            ),
            "see\n\nFiles:\n- /d/1/a.png\n- /d/2/b.txt"
        );
    }

    #[test]
    fn each_outcome_says_what_to_do_and_unknown_says_look_first() {
        assert_eq!(said(Outcome::Delivered, "alpha"), None);
        assert!(
            said(Outcome::Failed, "alpha")
                .unwrap()
                .contains("unchanged")
        );
        let unknown = said(Outcome::Unknown, "alpha").unwrap();
        assert!(unknown.contains("Look at the pane"), "{unknown}");
    }

    /// #242: Paste only said "Sent to"; it pasted, and sent nothing.
    #[test]
    fn an_arrival_says_whether_it_was_sent_or_only_pasted() {
        assert_eq!(arrived("alpha", false, true), "Sent to alpha");
        assert_eq!(arrived("alpha", false, false), "Pasted into alpha");
        assert!(arrived("alpha", true, false).starts_with("Launched alpha;"));
    }

    #[test]
    fn the_drawers_launch_gets_its_message_pasted_never_submitted() {
        let awaited = ("rig".to_string(), 4);
        let wait = Landed::Wait("$7".into());
        assert_eq!(landed(Some(&awaited), "rig", 4, true, "$7"), wait);
        assert_eq!(
            landed(
                Some(&awaited),
                "rig",
                4,
                true,
                "$7\0no completion notices: x"
            ),
            wait
        );
        assert_eq!(
            landed(Some(&awaited), "rig", 5, true, "$7"),
            Landed::NotOurs
        );
        assert_eq!(
            landed(Some(&awaited), "laptop", 4, true, "$7"),
            Landed::NotOurs
        );
        assert_eq!(landed(None, "rig", 4, true, "$7"), Landed::NotOurs);
        assert_eq!(landed(Some(&awaited), "rig", 4, false, ""), Landed::Failed);

        // Listed before its panes: not yet; another node's twin: never.
        let mut new = Session {
            node: "rig".into(),
            id: "$7".into(),
            name: "agent-7".into(),
            ..Default::default()
        };
        let twin = Session {
            node: "laptop".into(),
            panes: vec![Default::default()],
            ..new.clone()
        };
        assert_eq!(
            launch_paste(&[twin.clone(), new.clone()], "rig", "$7"),
            None
        );
        new.panes = vec![Default::default()];
        assert_eq!(
            launch_paste(&[twin, new.clone()], "rig", "$7"),
            Some((Key::of(&new), false))
        );
        assert_eq!(launch_paste(&[new], "rig", "$8"), None);
    }

    #[test]
    fn a_remembered_width_is_kept_within_the_room() {
        assert_eq!(width(None, 2000.), size::DRAWER);
        assert_eq!(width(Some(600.), 2000.), 600.);
        assert_eq!(width(Some(100.), 2000.), size::DRAWER_MIN);
        assert_eq!(width(Some(f32::NAN), 2000.), size::DRAWER);
        assert_eq!(width(Some(900.), 1000.), 520.);
        assert_eq!(width(Some(900.), 700.), size::DRAWER_MIN);
        assert_eq!(width(Some(700.), 1100.), 620.);
    }
}
