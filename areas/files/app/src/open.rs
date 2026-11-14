//! File viewer state and decoding rules.
use crate::store::{format, kinds, zoom};
use gpui::{ImageFormat, RenderImage};
use parvi_app_kit::widgets::markdown::{self, Block};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub fn version(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The most of a file shown as text: a long log reads whole.
pub const MAX_TEXT: usize = 16 << 20;
/// Text above this is read but not edited, rendered or laid out here.
pub const MAX_EDIT: usize = 1 << 20;

#[derive(Clone)]
pub enum Show {
    Reading,
    /// Text in the code view; Markdown also has its rendering.
    Text {
        markdown: Option<Vec<Block>>,
    },
    /// A picture, decoded, and its size in pixels.
    Picture {
        image: Arc<RenderImage>,
        natural: zoom::Px,
    },
    Model(super::model::Preview),
    /// Sound, video or a PDF, which this computer's own player or reader
    /// shows (the app has none of its own); not read until asked.
    Media(kinds::Kind),
    /// Why it is not shown inline, in a sentence.
    Not(String),
    Failed(String),
}

#[derive(Clone)]
pub struct Opened {
    pub node: String,
    pub path: String,
    pub name: String,
    pub size: Option<u64>,
    pub show: Show,
    /// The text as last saved or read, as the file has it.
    pub saved: String,
    /// The version of `saved` on the device ([`api::version`]), which a
    /// save names so the node refuses it over a later change.
    pub base: String,
    /// JSON laid out for reading, shown while not editing; editing and
    /// saving use the file's own text.
    pub pretty: Option<String>,
    pub editing: bool,
    pub saving: bool,
    /// Its text instead of its rendering: Markdown or JSON opened at a
    /// line.
    pub source: bool,
    /// What its repository says changed in it, when anything did.
    pub diff: Option<parvi_files_contract::Diff>,
    pub diff_open: bool,
    /// The file changed on disk under an unsaved edit.
    pub changed_on_disk: bool,
}

impl Opened {
    pub fn reading(node: String, path: String) -> Self {
        Self {
            name: super::store::paths::base_name(&path).to_owned(),
            node,
            path,
            size: None,
            show: Show::Reading,
            saved: String::new(),
            base: String::new(),
            pretty: None,
            editing: false,
            saving: false,
            source: false,
            diff: None,
            diff_open: false,
            changed_on_disk: false,
        }
    }

    pub fn key(&self) -> (String, String) {
        (self.node.clone(), self.path.clone())
    }

    /// Text small enough to edit here.
    pub fn editable(&self) -> bool {
        matches!(self.show, Show::Text { .. }) && self.saved.len() <= MAX_EDIT
    }
}

/// An edit not yet saved, and the version of the file it began from: a
/// save names that version, so a change made on the device meanwhile is
/// never overwritten unseen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub base: String,
}

/// After `sent` was saved: the edit still to keep, if the text is now
/// something else (typed while the save was on its way).
pub fn acknowledged(now: &str, sent: &str) -> Option<Draft> {
    (now != sent).then(|| Draft {
        text: now.to_string(),
        base: version(sent.as_bytes()),
    })
}

/// How a save is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Over the version the edit began from, and only that.
    Save,
    /// Over whatever the device has now: the operator chose it.
    Overwrite,
    /// Beside it, as a new file under a free name.
    Copy,
}

/// The name of the `n`th copy of `name` that keeps an edit.
pub fn copy_name(name: &str, n: usize) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    match n {
        1 => format!("{stem} (mine){ext}"),
        n => format!("{stem} (mine {n}){ext}"),
    }
}

pub fn picture_format(name: &str) -> Option<ImageFormat> {
    Some(match kinds::extension(name).as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif,
        "svg" => ImageFormat::Svg,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        _ => return None,
    })
}

/// What a read file other than a picture shows as, its text, and JSON laid
/// out for reading.
pub fn show(name: &str, bytes: &[u8]) -> (Show, String, Option<String>) {
    let size = format::size(bytes.len() as u64);
    let kind = kinds::kind(name);
    if bytes.len() > MAX_TEXT {
        return (
            Show::Not(format!("{size}, too large to show here")),
            String::new(),
            None,
        );
    }
    match kinds::as_text(bytes) {
        Some(text) => {
            let small = text.len() <= MAX_EDIT;
            let markdown = (small && kind == kinds::Kind::Markdown).then(|| markdown::parse(&text));
            let pretty = if small { pretty(name, &text) } else { None };
            (Show::Text { markdown }, text, pretty)
        }
        None => (
            Show::Not(format!("{size} of binary, not shown here")),
            String::new(),
            None,
        ),
    }
}

/// JSON laid out for reading, when that differs from its text: a JSON
/// file, or any file whose text is JSON on one line.
pub fn pretty(name: &str, text: &str) -> Option<String> {
    let body = text.trim();
    let one_line = !body.contains('\n') && (body.starts_with('{') || body.starts_with('['));
    if kinds::kind(name) != kinds::Kind::Json && !one_line {
        return None;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(text).ok()?;
    Some(lay_out(body)).filter(|p| p != text)
}

/// Valid JSON laid out two spaces to a level, its keys in their order.
fn lay_out(json: &str) -> String {
    let mut out = String::with_capacity(json.len() * 2);
    let mut depth = 0usize;
    let (mut quoted, mut escaped) = (false, false);
    let mut chars = json.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    while let Some(c) = chars.next() {
        if quoted {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if matches!(chars.peek(), Some('}' | ']')) {
                    out.push(chars.next().unwrap_or_default());
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_show_by_kind() {
        assert!(matches!(
            show("a.md", b"# hi").0,
            Show::Text { markdown: Some(_) }
        ));
        let (s, text, pretty) = show("a.json", br#"{"a":1}"#);
        assert!(matches!(s, Show::Text { markdown: None }));
        assert_eq!(text, r#"{"a":1}"#, "edited and saved as the file has it");
        assert_eq!(pretty.as_deref(), Some("{\n  \"a\": 1\n}"), "read laid out");
        assert_eq!(show("b.json", b"{\n  \"a\": 1\n}").2, None);
        assert_eq!(
            show("slot-0", br#"{"pid": 1, "note": "x"}"#).2.as_deref(),
            Some("{\n  \"pid\": 1,\n  \"note\": \"x\"\n}"),
            "JSON on one line is laid out whatever the name, its keys in order"
        );
        assert_eq!(
            show("a.json", br#"{"b":[1,{}],"a":"x, \"y\": {z}"}"#)
                .2
                .as_deref(),
            Some("{\n  \"b\": [\n    1,\n    {}\n  ],\n  \"a\": \"x, \\\"y\\\": {z}\"\n}")
        );
        assert_eq!(show("notes.txt", b"[not json").2, None);
        assert!(
            matches!(show("a.bin", &[0, 1, 2]).0, Show::Not(t) if t == "3 B of binary, not shown here")
        );
        assert!(matches!(show("notes", b"plain words").0, Show::Text { .. }));
    }

    #[test]
    fn a_save_answered_keeps_what_was_typed_after_it() {
        // Saved B over A, then typed back to A before the answer: A is
        // still an edit, of the version B.
        let kept = acknowledged("A", "B").expect("A differs from what was saved");
        assert_eq!(kept.text, "A");
        assert_eq!(kept.base, version(b"B"));
        assert_eq!(acknowledged("B", "B"), None);
    }

    #[test]
    fn a_copy_of_an_edit_takes_a_name_beside_the_file() {
        assert_eq!(copy_name("plan.md", 1), "plan (mine).md");
        assert_eq!(copy_name("plan.md", 3), "plan (mine 3).md");
        assert_eq!(copy_name("Makefile", 1), "Makefile (mine)");
        assert_eq!(copy_name(".env", 2), ".env (mine 2)");
    }
}
