//! The first exchange on every connection.
//!
//! The client opens with `parvi-holder <protocol>`. The holder answers
//! `parvi-holder <protocol> <version> <pid>` when it speaks that protocol, and
//! otherwise `%exit protocol <theirs> wanted, holder speaks <ours>` before
//! closing. After the greeting the connection is a tmux control-mode client.
//!
//! Sessions outlive a holder upgrade only if the next node speaks the running
//! holder's protocol, so the number changes only when the wire format does.

use anyhow::{Result, bail};

/// The wire format this build speaks.
pub const PROTOCOL: u32 = 2;

const WORD: &str = "parvi-holder";

/// What a running holder said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Greeting {
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
}

/// The client's opening line.
pub fn hello() -> String {
    format!("{WORD} {PROTOCOL}\n")
}

/// The holder's answer to an opening line: the greeting, or the reason to
/// refuse it.
pub fn answer(line: &str, pid: u32) -> Result<String, String> {
    let theirs = line
        .strip_prefix(WORD)
        .and_then(|rest| rest.trim().parse::<u32>().ok())
        .ok_or_else(|| format!("%exit not a parvi-holder client: {line:?}\n"))?;
    if theirs != PROTOCOL {
        return Err(format!(
            "%exit protocol {theirs} wanted, holder speaks {PROTOCOL}\n"
        ));
    }
    let version = env!("CARGO_PKG_VERSION");
    Ok(format!("{WORD} {PROTOCOL} {version} {pid}\n"))
}

/// Reads the holder's answer.
pub fn greeting(line: &str) -> Result<Greeting> {
    let line = line.trim_end();
    if let Some(reason) = line.strip_prefix("%exit ") {
        bail!("the holder refused: {reason}");
    }
    let mut words = line.split(' ');
    if words.next() != Some(WORD) {
        bail!("not a parvi holder: {line:?}");
    }
    let parsed = (|| {
        Some(Greeting {
            protocol: words.next()?.parse().ok()?,
            version: words.next()?.to_owned(),
            pid: words.next()?.parse().ok()?,
        })
    })();
    match parsed {
        Some(greeting) if greeting.protocol == PROTOCOL => Ok(greeting),
        Some(greeting) => bail!(
            "the holder speaks protocol {}, this build {PROTOCOL}",
            greeting.protocol
        ),
        None => bail!("bad greeting: {line:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_protocol_is_greeted() {
        let reply = answer(hello().trim_end(), 42).unwrap();
        let greeting = greeting(&reply).unwrap();
        assert_eq!(greeting.protocol, PROTOCOL);
        assert_eq!(greeting.pid, 42);
        assert_eq!(greeting.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn other_protocols_are_refused_with_both_numbers() {
        let refusal = answer("parvi-holder 999", 1).unwrap_err();
        assert_eq!(
            refusal,
            format!("%exit protocol 999 wanted, holder speaks {PROTOCOL}\n")
        );
        let error = greeting(&refusal).unwrap_err().to_string();
        assert!(error.contains("999"), "{error}");
        assert!(answer("ATTACH $1", 1).is_err());
    }

    #[test]
    fn a_newer_holder_is_named() {
        let error = greeting("parvi-holder 3 0.1.0 7").unwrap_err().to_string();
        assert!(error.contains("protocol 3"), "{error}");
    }
}
