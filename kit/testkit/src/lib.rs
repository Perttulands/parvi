//! What tests start: a node holding only the areas a test names
//! ([`node`]), a tmux server of the test's own ([`tmux`]), a whole
//! `parvi serve` process ([`process`]) and a stand-in for the sessions
//! ([`sessions`]).

pub mod node;
pub mod process;
pub mod sessions;
#[cfg(unix)]
pub mod tmux;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory only this test uses: freshly created, so nothing else was
/// there before, and private to the user running the tests.
pub fn fresh_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    loop {
        let dir = std::env::temp_dir().join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&dir) {
            Ok(()) => return dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("creating {}: {error}", dir.display()),
        }
    }
}
