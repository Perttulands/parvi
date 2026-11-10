//! Copy session, as CHROTE's `sessionCopy.ts` (#184): the same harness in
//! the same folder, on the same node and Unix user, under the next free
//! name in the source's family. Pure, so it is tested on its own.

use parvi_terminals_contract::{Harness, Session};

/// The launcher harness that runs what the session runs, or why a copy is
/// refused. A shell copies as the shell; an agent as the harness whose
/// program tmux reports in front.
pub fn harness<'a>(s: &Session, harnesses: &'a [Harness]) -> Result<&'a Harness, &'static str> {
    if s.cwd.as_deref().is_none_or(|c| c.trim().is_empty()) {
        return Err("Working directory unavailable");
    }
    let index = parvi_terminals_contract::session_copy::harness_index(
        s.command.as_deref().unwrap_or(""),
        harnesses.iter().map(|h| h.command.as_str()),
    )
    .ok_or("Running command is not a recognized harness")?;
    Ok(&harnesses[index])
}

/// The name a copy of `source` takes: `<family>-<n>` with the lowest free
/// `n` from 2. A copy of a copy stays in its family (`build-3` gives
/// `build-4`), but only while the family's first is listed: `project-2026`
/// alone is a name of its own, not a copy.
pub use parvi_terminals_contract::session_copy::next_name;

#[cfg(test)]
mod tests {
    use super::*;

    fn h(id: &str, command: &str) -> Harness {
        Harness {
            id: id.into(),
            command: command.into(),
            flags: String::new(),
        }
    }

    fn running(command: &str, cwd: Option<&str>) -> Session {
        Session {
            name: "s".into(),
            command: Some(command.into()),
            cwd: cwd.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn preserves_numeric_project_names_while_numbering_copies_of_a_known_family() {
        // CHROTE's sessionCopy.test.ts.
        assert_eq!(
            next_name("codex-project-2026", &["codex-project-2026"]),
            "codex-project-2026-2"
        );
        assert_eq!(
            next_name(
                "codex-project-3",
                &["codex-project", "codex-project-2", "codex-project-3"]
            ),
            "codex-project-4"
        );
        assert_eq!(next_name("build", &["build", "build-3"]), "build-2");
        assert_eq!(next_name("build-1", &["build", "build-1"]), "build-1-2");
    }

    #[test]
    fn copies_the_harness_that_runs_what_tmux_reports() {
        let list = [
            h("shell", ""),
            h("claude-code", "claude"),
            h("codex", "/usr/bin/codex"),
        ];
        let id = |s: &Session| harness(s, &list).map(|h| h.id.as_str());
        assert_eq!(id(&running("claude", Some("/srv"))), Ok("claude-code"));
        assert_eq!(id(&running("codex", Some("/srv"))), Ok("codex"));
        assert_eq!(id(&running("bash", Some("/srv"))), Ok("shell"));
        assert_eq!(id(&running("powershell.exe", Some("C:\\"))), Ok("shell"));
        assert_eq!(
            id(&running("node", Some("/srv"))),
            Err("Running command is not a recognized harness")
        );
        assert_eq!(
            id(&running("claude", None)),
            Err("Working directory unavailable")
        );
        assert_eq!(
            id(&running("claude", Some(" "))),
            Err("Working directory unavailable")
        );
        assert_eq!(
            id(&running("", Some("/srv"))),
            Err("Running command is not a recognized harness")
        );
    }
}
