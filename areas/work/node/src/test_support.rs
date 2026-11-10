/// Writes an executable script without this process ever holding it
/// open for writing. A file written here and run at once fails with
/// ETXTBSY whenever another test's `Command` forks while the write
/// handle is open: the forked child holds a copy until it execs, and
/// Linux refuses to run a file open for writing anywhere. Here the only
/// write handle is `cat`'s, in a child that has exited before the run.
pub(crate) fn write_script(path: &std::path::Path, text: &str) {
    use std::io::Write;
    let mut cat = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cat.stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(cat.wait().unwrap().success(), "writing {}", path.display());
}
