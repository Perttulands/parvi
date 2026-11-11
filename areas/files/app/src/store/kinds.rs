//! What a file can be shown as, from its name (CHROTE `fileKinds.ts`), and
//! whether bytes of no known kind are text (CHROTE `probeTextFile`).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Json,
    Text,
    Image,
    Audio,
    Video,
    Pdf,
    Model,
    Other,
}

const TEXT: &[&str] = &[
    "adoc",
    "asm",
    "bash",
    "bat",
    "c",
    "cc",
    "cfg",
    "clj",
    "cljs",
    "cljc",
    "cmake",
    "cnf",
    "conf",
    "cpp",
    "cs",
    "css",
    "csv",
    "cts",
    "cue",
    "dart",
    "desktop",
    "diff",
    "dockerfile",
    "dockerignore",
    "editorconfig",
    "eml",
    "env",
    "erl",
    "ex",
    "exs",
    "fish",
    "fs",
    "fsx",
    "geojson",
    "gitattributes",
    "gitconfig",
    "gitignore",
    "gitmodules",
    "go",
    "gql",
    "gradle",
    "graphql",
    "groovy",
    "h",
    "har",
    "hcl",
    "hpp",
    "hs",
    "htm",
    "html",
    "ics",
    "ignore",
    "ini",
    "ipynb",
    "java",
    "jl",
    "js",
    "json5",
    "jsonc",
    "jsonl",
    "jsx",
    "kt",
    "kts",
    "less",
    "lock",
    "log",
    "lua",
    "m",
    "mjs",
    "mount",
    "ndjson",
    "nim",
    "nix",
    "npmrc",
    "patch",
    "path",
    "pem",
    "php",
    "pl",
    "properties",
    "proto",
    "ps1",
    "psv",
    "py",
    "r",
    "rb",
    "rego",
    "rs",
    "rst",
    "s",
    "sass",
    "scala",
    "scss",
    "service",
    "sh",
    "socket",
    "sql",
    "svelte",
    "swift",
    "target",
    "tex",
    "tf",
    "tfstate",
    "tfvars",
    "timer",
    "toml",
    "ts",
    "tsbuildinfo",
    "tsv",
    "tsx",
    "txt",
    "vcf",
    "vue",
    "webmanifest",
    "xml",
    "yaml",
    "yml",
    "zsh",
];
const TEXT_NAMES: &[&str] = &[
    "brewfile",
    "gemfile",
    "go.mod",
    "go.sum",
    "go.work",
    "jenkinsfile",
    "justfile",
    "license",
    "makefile",
    "pipfile",
    "procfile",
    "rakefile",
    "readme",
    "vagrantfile",
];
/// Pictures the app draws itself (gpui's decoders).
const IMAGE: &[&str] = &[
    "bmp", "gif", "ico", "jpeg", "jpg", "png", "svg", "tif", "tiff", "webp",
];

const AUDIO: &[&str] = &["aac", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav"];
const VIDEO: &[&str] = &[
    "avi", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "ogv", "webm",
];

pub fn extension(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower == "dockerfile" || lower.starts_with("dockerfile.") {
        return "dockerfile".into();
    }
    if lower.starts_with(".env") {
        return "env".into();
    }
    match lower.rsplit_once('.') {
        Some((_, ext)) => ext.to_string(),
        None => String::new(),
    }
}

pub fn kind(name: &str) -> Kind {
    let ext = extension(name);
    match ext.as_str() {
        "md" | "markdown" => Kind::Markdown,
        "json" => Kind::Json,
        "glb" => Kind::Model,
        "pdf" => Kind::Pdf,
        e if IMAGE.contains(&e) => Kind::Image,
        e if AUDIO.contains(&e) => Kind::Audio,
        e if VIDEO.contains(&e) => Kind::Video,
        e if TEXT.contains(&e) || TEXT_NAMES.contains(&name.to_lowercase().as_str()) => Kind::Text,
        _ => Kind::Other,
    }
}

/// Bytes as text, or `None` when they look binary: a NUL, invalid UTF-8, or
/// a control character other than tab, line ends and escape.
pub fn as_text(bytes: &[u8]) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let binary = text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x1b' | '\x0c'));
    (!binary).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_name() {
        assert_eq!(kind("README.md"), Kind::Markdown);
        assert_eq!(kind("a.JSON"), Kind::Json);
        assert_eq!(kind("Makefile"), Kind::Text);
        assert_eq!(kind("x.png"), Kind::Image);
        assert_eq!(kind("hero.glb"), Kind::Model);
        assert_eq!(kind("voice.MP3"), Kind::Audio);
        assert_eq!(kind("clip.webm"), Kind::Video);
        assert_eq!(kind("paper.pdf"), Kind::Pdf);
        assert_eq!(kind("data.bin"), Kind::Other);
        assert_eq!(kind(".env.local"), Kind::Text);
    }

    #[test]
    fn binary_is_not_text() {
        assert_eq!(as_text(b"hi\tthere\n").as_deref(), Some("hi\tthere\n"));
        assert!(as_text(b"a\0b").is_none());
        assert!(as_text(&[0xff, 0xfe]).is_none());
        assert!(as_text(b"\x01\x02").is_none());
    }
}
