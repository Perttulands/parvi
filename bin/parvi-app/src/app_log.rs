//! The app's log: gpui's warnings and errors, on stderr and in `app.log`
//! beside `app.json` (a Windows program has no console). Failed desktop
//! notifications are logged here along with other gpui warnings and errors.

use std::io::Write;
use std::path::PathBuf;

struct Log {
    path: Option<PathBuf>,
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
        if let Some(path) = &self.path
            && let Ok(mut file) = std::fs::File::options()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    fn flush(&self) {}
}

/// Installs the log.
pub fn install() {
    let logger = Log {
        path: parvi_app_kit::settings::path().map(|p| p.with_file_name("app.log")),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_notification_is_written_to_the_log() {
        use log::Log as _;
        let path = std::env::temp_dir().join(format!("parvi-app-log-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let logger = Log {
            path: Some(path.clone()),
        };
        logger.log(
            &log::Record::builder()
                .level(log::Level::Warn)
                .target("gpui")
                .args(format_args!(
                    "failed to show system notification: unavailable"
                ))
                .build(),
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "parvi-app WARN gpui: failed to show system notification: unavailable\n"
        );
        std::fs::remove_file(path).unwrap();
    }
}
