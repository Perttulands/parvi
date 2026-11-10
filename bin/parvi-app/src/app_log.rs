//! The app's log: gpui's warnings and errors, on stderr and in `app.log`
//! beside `app.json` (a Windows program has no console). gpui posts a
//! desktop notification without saying whether it arrived and logs the
//! failure instead, so the log is also where a failed notification is
//! caught and handed to the window, which says so in the status line.

use std::io::Write;
use std::sync::OnceLock;

static FAILED: OnceLock<async_channel::Sender<String>> = OnceLock::new();

struct Log;

/// What gpui logs when a notification could not be shown: on Linux from its
/// own thread ("failed to show system notification: …"), or for want of an
/// app identity.
fn notification_failed(text: &str) -> bool {
    (text.starts_with("failed") || text.starts_with("cannot"))
        && text.contains("system notification")
}

impl log::Log for Log {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let text = r.args().to_string();
        let line = format!("parvi-app {} {}: {text}", r.level(), r.target());
        eprintln!("{line}");
        if let Some(path) = parvi_app_kit::settings::path().map(|p| p.with_file_name("app.log"))
            && let Ok(mut file) = std::fs::File::options()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
        if notification_failed(&text)
            && let Some(tx) = FAILED.get()
        {
            let _ = tx.try_send(text);
        }
    }

    fn flush(&self) {}
}

/// Installs the log. Failed desktop notifications arrive on the receiver.
pub fn install() -> async_channel::Receiver<String> {
    let (tx, rx) = async_channel::unbounded();
    let _ = FAILED.set(tx);
    if log::set_logger(&Log).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_notification_is_told_from_other_warnings() {
        assert!(notification_failed(
            "failed to show system notification: org.freedesktop.DBus.Error.ServiceUnknown"
        ));
        assert!(notification_failed(
            "cannot show a system notification without an app identity; call `App::set_app_identity` during startup"
        ));
        assert!(!notification_failed(
            "system notification returned unknown action \"x\""
        ));
        assert!(!notification_failed("failed to load font"));
    }
}
