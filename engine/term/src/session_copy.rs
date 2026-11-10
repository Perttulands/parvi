//! Shared session copy selection rules.

/// Find the launcher entry whose command matches the running program.
/// Shells match an empty launcher command. Paths and Windows executables are normalized.
pub fn harness_index<'a>(
    running: &str,
    mut commands: impl Iterator<Item = &'a str>,
) -> Option<usize> {
    fn program(s: &str) -> String {
        let name = s
            .trim()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        name.strip_suffix(".exe").unwrap_or(&name).to_owned()
    }
    let running = program(running);
    if running.is_empty() {
        return None;
    }
    let shell = [
        "sh",
        "bash",
        "dash",
        "fish",
        "ksh",
        "zsh",
        "pwsh",
        "powershell",
        "cmd",
    ]
    .contains(&running.as_str());
    commands.position(|command| {
        if shell {
            command.is_empty()
        } else {
            let first = command.split_whitespace().next().unwrap_or("");
            !first.is_empty() && program(first) == running
        }
    })
}

/// Choose the lowest free family name for a copy.
pub fn next_name(source: &str, taken: &[&str]) -> String {
    let base = match source.rsplit_once('-') {
        Some((stem, n))
            if !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && n.parse::<u64>().is_ok_and(|n| n >= 2)
                && taken.contains(&stem) =>
        {
            stem
        }
        _ => source,
    };
    (2u64..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(&c.as_str()))
        .expect("a free name")
}

#[cfg(test)]
mod harness_tests {
    use super::harness_index;
    #[test]
    fn matches_shells_and_harnesses_across_paths_and_refuses_unknown() {
        let commands = ["", "/usr/bin/claude --flag", "C:\\tools\\codex.exe"];
        for (running, expected) in [
            ("bash", Some(0)),
            ("PowerShell.EXE", Some(0)),
            ("claude", Some(1)),
            ("codex.exe", Some(2)),
            ("node", None),
            ("", None),
        ] {
            assert_eq!(harness_index(running, commands.iter().copied()), expected);
        }
    }
}
