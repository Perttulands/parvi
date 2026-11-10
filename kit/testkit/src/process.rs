//! `parvi serve` processes a test started, trusted only once they say they
//! serve where the test asked.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// A `parvi serve` this test started, killed when dropped.
pub struct Node {
    pub child: Child,
    /// Where the node said it serves.
    pub address: SocketAddr,
}

impl Node {
    /// Starts `command`, a `parvi serve` without `--listen`, on `listen`, and
    /// returns once the node reports serving there. Port 0 takes the port the
    /// node binds, so no port is picked and released before it binds. A node
    /// that exits first, as one that cannot bind does, fails the test rather
    /// than leaving it to reach whatever answers on that port.
    pub fn start(command: &mut Command, listen: SocketAddr) -> Node {
        let mut child = command
            .arg("--listen")
            .arg(listen.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start parvi serve");
        let stderr = child.stderr.take().unwrap();
        let (served, serving) = mpsc::channel();
        // Its diagnostics stay in the test's output, and reading on keeps the
        // pipe from filling.
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                eprintln!("{line}");
                if let Some(address) = serving_on(&line) {
                    let _ = served.send(address);
                }
            }
        });
        let mut node = Node {
            child,
            address: listen,
        };
        match serving.recv_timeout(Duration::from_secs(30)) {
            Ok(address) => {
                assert!(
                    address.ip() == listen.ip()
                        && (listen.port() == 0 || address.port() == listen.port()),
                    "parvi serve asked for {listen} serves on {address}"
                );
                node.address = address;
                node
            }
            Err(RecvTimeoutError::Disconnected) => panic!(
                "parvi serve on {listen} exited before serving: {:?}",
                node.child.wait()
            ),
            Err(RecvTimeoutError::Timeout) => {
                panic!("parvi serve on {listen} did not serve within 30 s")
            }
        }
    }

    /// Kills the node and waits until it has exited.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The address in `parvi: node ID (DEVICE, ENVIRONMENT) serving http://ADDRESS`.
fn serving_on(line: &str) -> Option<SocketAddr> {
    line.strip_prefix("parvi: node ")?
        .rsplit_once(" serving http://")?
        .1
        .parse()
        .ok()
}
