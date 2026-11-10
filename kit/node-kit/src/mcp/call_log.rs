//! Every tool call, without recording input or error details.

use super::rpc::Caller;

pub(super) fn record(caller: &Caller, tool: &str, target: &str, outcome: &str) {
    eprintln!("{}", line(caller, tool, target, outcome));
}

fn line(caller: &Caller, tool: &str, target: &str, outcome: &str) -> String {
    format!(
        "parvi: mcp {:?} {tool} target={:?} {outcome}",
        caller.who, target
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_only_safe_fields() {
        let caller = Caller {
            who: "client".into(),
        };
        assert_eq!(
            line(&caller, "session_send", "node-a", "failed"),
            "parvi: mcp \"client\" session_send target=\"node-a\" failed"
        );
    }
}
