use super::{conversation, launch, tokens, transcript};
use parvi_term::{
    Screen,
    waiting::harness::{self, Harness},
};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Posix,
    PowerShell,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotBinding {
    pub model: String,
    pub effort: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub command: String,
    /// Executable and arguments for an owned terminal session.
    pub argv: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys(pub String);
#[derive(Debug, Clone, Default)]
pub struct TranscriptRoots {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub pi: PathBuf,
}
impl TranscriptRoots {
    /// Native transcript locations, respecting the harnesses' environment overrides.
    pub fn from_host() -> Self {
        let home = crate::store::home_dir().unwrap_or_default();
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex = std::env::var_os("CODEX_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let pi = std::env::var_os("PI_CODING_AGENT_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi/agent"));
        Self {
            claude: claude.join("projects"),
            codex: codex.join("sessions"),
            pi: pi.join("sessions"),
        }
    }
}
/// A dispatched native user message. Once discovered, retain the transcript
/// path and session ID; this prevents a later unrelated session being chosen.
#[derive(Debug, Clone, Default)]
pub struct Pointer {
    pub text: String,
    pub cwd: PathBuf,
    pub transcript: Option<PathBuf>,
    pub session_id: Option<String>,
    pub pid: Option<u32>,
    pub known_threads: BTreeSet<PathBuf>,
    /// Ignore transcripts last written before this dispatch was prepared.
    pub not_before: Option<std::time::SystemTime>,
    /// The model and effort selected for this dispatch. Required for completion.
    pub binding: Option<SlotBinding>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: i64,
    pub cache_write: i64,
    pub cache_read: i64,
    pub output: i64,
}
impl TokenUsage {
    pub fn counted(self) -> i64 {
        self.input + self.cache_write + self.output
    }
    pub(super) fn add(&mut self, other: Self) {
        self.input += other.input;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.output += other.output;
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnState {
    Pending,
    Completed { text: String },
    EndedWithoutSentinel,
    BackgroundWork,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub state: TurnState,
    pub consumed: bool,
    pub session_id: String,
    pub model: String,
    pub effort: String,
    pub operator_turns: usize,
    pub transcript: Option<PathBuf>,
}
impl Default for Turn {
    fn default() -> Self {
        Self {
            state: TurnState::Pending,
            consumed: false,
            session_id: String::new(),
            model: String::new(),
            effort: String::new(),
            operator_turns: 0,
            transcript: None,
        }
    }
}

pub trait HarnessAdapter: Send + Sync {
    fn launch(
        &self,
        binding: &SlotBinding,
        cwd: &Path,
        shell: Shell,
        roots: &TranscriptRoots,
    ) -> io::Result<Launch>;
    fn ready(&self, screen: &Screen) -> bool;
    fn working(&self, screen: &Screen) -> bool;
    /// Recognition is valid only before a brief is sent and when no input is active.
    fn trust_dialog(&self, screen: &Screen) -> Option<Keys>;
    fn turn(&self, roots: &TranscriptRoots, pointer: &Pointer, sentinel: &str) -> io::Result<Turn>;
    /// `None` means the process cannot be inspected on this platform.
    fn conversation_replaced(
        &self,
        roots: &TranscriptRoots,
        pointer: &Pointer,
    ) -> io::Result<Option<bool>>;
    fn tokens_since(&self, pointer: &Pointer) -> io::Result<TokenUsage>;
}
pub struct Claude;
pub struct Codex;
pub struct Pi;
macro_rules! adapter {
    ($name:ident, $kind:expr) => {
        impl HarnessAdapter for $name {
            fn launch(
                &self,
                binding: &SlotBinding,
                cwd: &Path,
                shell: Shell,
                roots: &TranscriptRoots,
            ) -> io::Result<Launch> {
                launch::render($kind, binding, cwd, shell, roots)
            }
            fn ready(&self, screen: &Screen) -> bool {
                harness::ready($kind, screen)
            }
            fn working(&self, screen: &Screen) -> bool {
                harness::working($kind, screen)
            }
            fn trust_dialog(&self, screen: &Screen) -> Option<Keys> {
                if !harness::trust_dialog($kind, screen) {
                    return None;
                }
                // Claude may initially select “No”; wait for the next screen
                // after moving the selection before asking for Enter.
                let selected = screen
                    .rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .filter(|cell| cell.width != 0)
                            .map(|cell| {
                                if cell.text.is_empty() {
                                    " "
                                } else {
                                    cell.text.as_str()
                                }
                            })
                            .collect::<String>()
                    })
                    .any(|line| {
                        line.contains("❯ Yes, I trust this folder")
                            || line.contains("❯ 1. Yes, I trust this folder")
                    });
                Some(Keys(
                    if $kind == Harness::Claude && !selected {
                        "Down"
                    } else {
                        "Enter"
                    }
                    .into(),
                ))
            }
            fn turn(
                &self,
                roots: &TranscriptRoots,
                pointer: &Pointer,
                sentinel: &str,
            ) -> io::Result<Turn> {
                transcript::find($kind, roots, pointer, sentinel)
            }
            fn conversation_replaced(
                &self,
                roots: &TranscriptRoots,
                pointer: &Pointer,
            ) -> io::Result<Option<bool>> {
                conversation::replaced($kind, roots, pointer)
            }
            fn tokens_since(&self, pointer: &Pointer) -> io::Result<TokenUsage> {
                tokens::read($kind, pointer)
            }
        }
    };
}
adapter!(Claude, Harness::Claude);
adapter!(Codex, Harness::Codex);
adapter!(Pi, Harness::Pi);
