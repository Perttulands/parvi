//! Names used in the seat's output contract.
pub const VERDICT: &str = "parvi-verdict";
pub const OUTPUTS: &str = "parvi-outputs";
pub const PICK: &str = "parvi-pick";
pub const DONE: &str = "<<<PARVI-DONE ";

/// The fields of a sentinel whose opening `DONE` has been cut off, and the
/// text after it. A sentinel ends at `>>>`; one an agent left unclosed ends
/// with its line, so a dropped closer does not leave a finished seat running.
pub fn sentinel_fields(tail: &str) -> (&str, &str) {
    let (line, after_line) = tail.split_once('\n').unwrap_or((tail, ""));
    match line.split_once(">>>") {
        Some((fields, _)) => (fields, &tail[fields.len() + 3..]),
        None => (line, after_line),
    }
}
pub const ESCALATE: &str = "<<<PARVI-ESCALATE ";
