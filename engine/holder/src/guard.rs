//! The guarded submit form of tmux's `if-shell -F` used by Send.
//!
//! Only the engine's exact comparison and branches are accepted: the holder
//! does not execute arbitrary shell commands supplied over its pipe.

use crate::words::{Args, split};

#[derive(Debug, PartialEq, Eq)]
pub struct Guard {
    pub pane: u32,
    pub pid: u32,
}

pub fn parse(words: &[String]) -> Result<Guard, String> {
    let args = Args::parse(words, "t");
    if args.flags.len() != 2 || !args.has('F') || args.get('t').is_none() || args.rest.len() != 3 {
        return Err("unsupported if-shell".into());
    }
    let target = args.get('t').unwrap_or_default();
    let pane = target
        .strip_prefix('%')
        .and_then(|n| n.parse::<u32>().ok())
        .ok_or("invalid if-shell pane")?;
    let pid = args.rest[0]
        .strip_prefix("#{==:#{pane_pid},")
        .and_then(|s| s.strip_suffix('}'))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or("unsupported if-shell condition")?;
    if split(&args.rest[1]) != ["send-keys", "-t", target, "Enter"]
        || split(&args.rest[2]) != ["display-message", "-p", "target-changed"]
    {
        return Err("unsupported if-shell branches".into());
    }
    Ok(Guard { pane, pid })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_guarded_submit_contract() {
        let words = split(
            "-F -t %7 '#{==:#{pane_pid},123}' 'send-keys -t %7 Enter' 'display-message -p target-changed'",
        );
        assert_eq!(parse(&words), Ok(Guard { pane: 7, pid: 123 }));
        for text in [
            "-t %7 '#{==:#{pane_pid},123}' 'send-keys -t %7 Enter' 'display-message -p target-changed'",
            "-F -t %7 '#{==:#{pane_pid},123}' 'send-keys -t %8 Enter' 'display-message -p target-changed'",
            "-F -t %7 '#{==:#{pane_pid},no}' 'send-keys -t %7 Enter' 'display-message -p target-changed'",
            "-F -t %7 '#{==:#{pane_pid},123}' 'kill-server' 'display-message -p target-changed'",
        ] {
            assert!(parse(&split(text)).is_err(), "{text}");
        }
    }
}
