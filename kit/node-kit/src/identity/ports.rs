//! The port a node of each environment listens on. WSL forwards loopback
//! ports to Windows, so the two nodes of one Windows machine need different
//! ports.

use anyhow::{Context, Result, bail};

use super::Environment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    linux: u16,
    wsl: u16,
    windows: u16,
}

impl Default for Ports {
    fn default() -> Ports {
        Ports {
            linux: 7300,
            wsl: 7300,
            windows: 7301,
        }
    }
}

impl Ports {
    /// Takes `environment=port`, as `[node] ports` gives it.
    pub fn set(&mut self, setting: &str) -> Result<()> {
        let (environment, port) = setting
            .split_once('=')
            .context("a port is environment=port")?;
        let port = port
            .parse()
            .with_context(|| format!("{setting}: not a port"))?;
        *match environment {
            "linux" => &mut self.linux,
            "wsl" => &mut self.wsl,
            "windows" => &mut self.windows,
            _ => bail!("{setting}: the environment is linux, wsl or windows"),
        } = port;
        Ok(())
    }

    pub fn of(&self, environment: Environment) -> u16 {
        match environment {
            Environment::Linux => self.linux,
            Environment::Wsl => self.wsl,
            Environment::Windows => self.windows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_and_wsl_differ_by_default() {
        let ports = Ports::default();
        assert_ne!(ports.of(Environment::Wsl), ports.of(Environment::Windows));
        assert_eq!(ports.of(Environment::Linux), 7300);
    }

    #[test]
    fn each_environment_can_be_moved() {
        let mut ports = Ports::default();
        ports.set("windows=7400").unwrap();
        ports.set("linux=7500").unwrap();
        assert_eq!(ports.of(Environment::Windows), 7400);
        assert_eq!(ports.of(Environment::Linux), 7500);
        assert_eq!(ports.of(Environment::Wsl), 7300);
        for bad in ["windows", "mac=1", "wsl=x", "wsl=70000"] {
            assert!(ports.set(bad).is_err(), "{bad}");
        }
    }
}
