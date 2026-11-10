//! Which environment of its device a node is.

/// What a node runs in. A Windows machine with WSL runs two nodes, one per
/// environment, and they show as one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Linux,
    Wsl,
    Windows,
}

impl Environment {
    /// The environment this process runs in.
    pub fn detect() -> Environment {
        if cfg!(windows) {
            return Environment::Windows;
        }
        let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
        Environment::from_release(&release)
    }

    /// Linux or WSL from the kernel's release string: WSL's kernels name
    /// Microsoft (`5.15.153.1-microsoft-standard-WSL2`, `4.4.0-19041-Microsoft`).
    fn from_release(release: &str) -> Environment {
        if release.to_ascii_lowercase().contains("microsoft") {
            Environment::Wsl
        } else {
            Environment::Linux
        }
    }

    /// The name the hello carries.
    pub fn name(self) -> &'static str {
        match self {
            Environment::Linux => "linux",
            Environment::Wsl => "wsl",
            Environment::Windows => "windows",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_wsl_from_linux_by_the_kernel() {
        let from = Environment::from_release;
        assert_eq!(
            from("5.15.153.1-microsoft-standard-WSL2\n"),
            Environment::Wsl
        );
        assert_eq!(from("4.4.0-19041-Microsoft"), Environment::Wsl);
        assert_eq!(from("6.8.0-45-generic\n"), Environment::Linux);
        assert_eq!(from(""), Environment::Linux);
    }
}
