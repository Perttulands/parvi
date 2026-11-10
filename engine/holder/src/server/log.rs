//! The holder's log. It runs with no console, so this is where its life and
//! its failures show: start, sessions starting and ending, and errors.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use super::client::now;

pub struct Log {
    path: PathBuf,
    write: Mutex<()>,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Log {
            path,
            write: Mutex::new(()),
        }
    }

    pub fn line(&self, message: &str) {
        let _guard = self.write.lock().unwrap();
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{} [{}] {message}", now(), std::process::id());
        }
    }
}
