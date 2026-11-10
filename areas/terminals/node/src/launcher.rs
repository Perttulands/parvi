//! The launcher's file, `launch.conf`: read when the node starts, where a
//! file that does not parse stops it, so a typo can't quietly drop a
//! harness; then looked at again (its size and time, a stat) whenever a
//! client connects. A change reaches every client's launcher frame; a bad
//! edit keeps the last good launcher and says why.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::launch::Config;

/// What a file looked like: its time and size, `None` without one.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[derive(Default)]
struct State {
    stamp: Stamp,
    error: Option<String>,
}

pub struct Launcher {
    path: PathBuf,
    state: Mutex<State>,
    config: watch::Sender<Arc<Config>>,
}

impl Launcher {
    /// Reads `path`; no file there is the default launcher, until one is
    /// written.
    pub fn load(path: PathBuf) -> Result<Launcher> {
        let config = match std::fs::read_to_string(&path) {
            Ok(text) => Config::parse(&text).with_context(|| format!("in {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        let launcher = Launcher {
            state: Mutex::new(State {
                stamp: stamp(&path),
                error: None,
            }),
            path,
            config: watch::Sender::new(Arc::new(config)),
        };
        Ok(launcher)
    }

    /// The launcher as the file last said it.
    pub fn get(&self) -> Arc<Config> {
        self.config.borrow().clone()
    }

    /// Changes whenever the launcher does.
    pub fn watch(&self) -> watch::Receiver<Arc<Config>> {
        self.config.subscribe()
    }

    /// Why the file as it is now is not the launcher, if it isn't.
    #[cfg(test)]
    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }

    /// Reads the file if it changed since the last look, and tells every
    /// watcher.
    pub fn look(&self) {
        let path = &self.path;
        let mut state = self.state.lock().unwrap();
        let now = stamp(path);
        if now == state.stamp {
            return;
        }
        state.stamp = now;
        let read = match now {
            None => Ok(Config::default()),
            Some(_) => std::fs::read_to_string(path)
                .map_err(anyhow::Error::from)
                .and_then(|text| Config::parse(&text)),
        };
        match read {
            Ok(config) => {
                state.error = None;
                self.config.send_if_modified(|c| {
                    let new = **c != config;
                    *c = Arc::new(config);
                    new
                });
            }
            Err(error) => {
                let error = format!("{error:#}");
                eprintln!(
                    "parvi: {}: {error}; the launcher stays as it was",
                    path.display()
                );
                state.error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute folder on this platform.
    const SRV: &str = if cfg!(windows) { r"C:\srv" } else { "/srv" };

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("parvi-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_edit_made_in_the_file_reaches_the_launcher_and_a_broken_one_is_reported() {
        let dir = scratch("file");
        let path = dir.join("launch.conf");
        std::fs::write(&path, "harness codex codex\n").unwrap();
        let s = Launcher::load(path.clone()).unwrap();
        let mut launcher = s.watch();
        std::fs::write(
            &path,
            format!("harness codex codex --full-auto\nfolder {SRV}\n"),
        )
        .unwrap();
        s.look();
        assert!(launcher.has_changed().unwrap());
        assert_eq!(
            launcher.borrow_and_update().harnesses[1].flags,
            "--full-auto"
        );
        std::fs::write(&path, "harness codex\n").unwrap();
        s.look();
        assert!(s.error().unwrap().contains("needs a command"));
        // The last good launcher stays.
        assert_eq!(s.get().folders, [SRV]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_does_not_parse_stops_the_start_and_none_is_the_default() {
        let dir = scratch("start");
        let path = dir.join("launch.conf");
        assert_eq!(
            *Launcher::load(path.clone()).unwrap().get(),
            Config::default()
        );
        std::fs::write(&path, "colour red\n").unwrap();
        assert!(Launcher::load(path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
