//! The machine's name, which the trust checks accept and the node names its
//! device by.

#[cfg(unix)]
pub(crate) fn hostname() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname writes at most that.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if status != 0 {
        return None;
    }
    let end = buffer.iter().position(|&b| b == 0)?;
    String::from_utf8(buffer[..end].to_vec()).ok()
}

/// Windows keeps the NetBIOS name in capitals; the WSL node and the tailnet
/// name the same machine in lower case, and one device needs one name.
#[cfg(windows)]
pub(crate) fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .map(|name| name.to_lowercase())
}
