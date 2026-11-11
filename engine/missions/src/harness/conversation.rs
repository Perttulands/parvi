use super::{
    adapter::{Pointer, TranscriptRoots},
    transcript::s,
};
use parvi_term::waiting::harness::Harness;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

/// Capture these at dispatch time, along with the selected rollout, so a
/// later writable user thread can be distinguished from existing descriptors.
#[cfg(target_os = "linux")]
pub fn codex_threads(root: &Path, pid: u32) -> Result<BTreeSet<PathBuf>, crate::Error> {
    codex_threads_in(root, pid, Path::new("/proc")).map_err(crate::error::io)
}
#[cfg(target_os = "linux")]
fn codex_threads_in(root: &Path, pid: u32, proc: &Path) -> io::Result<BTreeSet<PathBuf>> {
    let base = proc.join(pid.to_string());
    let mut found = BTreeSet::new();
    let entries = match fs::read_dir(base.join("fd")) {
        Ok(entries) => entries,
        Err(e)
            if e.kind() == io::ErrorKind::NotFound
                || e.kind() == io::ErrorKind::PermissionDenied =>
        {
            return Ok(found);
        }
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let Ok(path) = fs::read_link(entry.path()) else {
            continue;
        };
        if !path.starts_with(root) || path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Ok(info) = fs::read_to_string(base.join("fdinfo").join(entry.file_name())) else {
            continue;
        };
        let flags = info
            .lines()
            .find_map(|line| {
                line.strip_prefix("flags:")
                    .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
            })
            .unwrap_or(0);
        if flags & 3 == 0 {
            continue;
        }
        if codex_user_thread(&path) {
            found.insert(path);
        }
    }
    Ok(found)
}
#[cfg(target_os = "linux")]
fn codex_user_thread(path: &Path) -> bool {
    use std::io::BufRead;
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut line = Vec::new();
    if io::BufReader::new(file)
        .read_until(b'\n', &mut line)
        .is_err()
    {
        return false;
    }
    let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line) else {
        return false;
    };
    s(&row, "type") == "session_meta" && ["", "user"].contains(&s(&row["payload"], "thread_source"))
}
#[cfg(not(target_os = "linux"))]
pub fn codex_threads(_root: &Path, _pid: u32) -> Result<BTreeSet<PathBuf>, crate::Error> {
    (|| -> io::Result<BTreeSet<PathBuf>> { Ok(BTreeSet::new()) })().map_err(crate::error::io)
}
pub(super) fn replaced(
    h: Harness,
    roots: &TranscriptRoots,
    pointer: &Pointer,
) -> io::Result<Option<bool>> {
    let Some(pid) = pointer.pid else {
        return Ok(None);
    };
    match h {
        Harness::Claude => {
            let Some(session) = &pointer.session_id else {
                return Ok(None);
            };
            let path = roots
                .claude
                .parent()
                .unwrap_or(&roots.claude)
                .join("sessions")
                .join(format!("{pid}.json"));
            let Ok(bytes) = fs::read(path) else {
                return Ok(None);
            };
            let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                return Ok(None);
            };
            let id = s(&json, "sessionId");
            if id.is_empty() {
                Ok(None)
            } else {
                Ok(Some(id != session))
            }
        }
        Harness::Codex => {
            #[cfg(target_os = "linux")]
            {
                let threads = codex_threads(&roots.codex, pid).map_err(crate::error::to_io)?;
                Ok(Some(threads.iter().any(|path| {
                    !pointer.known_threads.contains(path)
                        && pointer.transcript.as_ref() != Some(path)
                })))
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = pid;
                Ok(None)
            }
        }
        Harness::Pi => Ok(None),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn only_writable_user_threads_are_active() {
        let base = std::env::temp_dir().join(format!("parvi-104-proc-{}", std::process::id()));
        let root = base.join("sessions");
        let proc = base.join("proc");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(proc.join("42/fd")).unwrap();
        fs::create_dir_all(proc.join("42/fdinfo")).unwrap();
        for (fd, source, flags) in [
            ("1", "user", "02000002"),
            ("2", "user", "02000000"),
            ("3", "subagent", "02000002"),
        ] {
            let path = root.join(format!("{fd}.jsonl"));
            fs::write(
                &path,
                format!(
                    "{{\"type\":\"session_meta\",\"payload\":{{\"thread_source\":\"{source}\"}}}}\n"
                ),
            )
            .unwrap();
            symlink(&path, proc.join("42/fd").join(fd)).unwrap();
            fs::write(
                proc.join("42/fdinfo").join(fd),
                format!("flags:\t{flags}\n"),
            )
            .unwrap();
        }
        assert_eq!(
            codex_threads_in(&root, 42, &proc).unwrap(),
            BTreeSet::from([root.join("1.jsonl")])
        );
        fs::remove_dir_all(base).unwrap();
    }
}
