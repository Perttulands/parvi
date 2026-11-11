//! Names the build: `PARVI_VERSION` is the package version and, when built
//! from a Git checkout, the commit (`0.0.1+8cdb58b`), so two builds of one
//! version tell apart in `/api/health` and `parvi --version`.

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let commit = git(&["rev-parse", "--short=7", "HEAD"]);
    println!(
        "cargo:rustc-env=PARVI_VERSION={}",
        match &commit {
            Some(commit) => format!("{version}+{commit}"),
            None => version,
        }
    );
    // A new commit moves HEAD or the branch it names; the build follows.
    if commit.is_some() {
        let mut watched = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
        watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
        for name in watched {
            // A path that does not exist would rerun this on every build.
            let path = git(&["rev-parse", "--path-format=absolute", "--git-path", &name])
                .map(PathBuf::from)
                .filter(|path| path.exists());
            if let Some(path) = path {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    } else {
        println!("cargo:rerun-if-changed=build.rs");
    }
}
