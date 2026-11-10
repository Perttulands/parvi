//! A tmux control-mode client spoken in-process over tmux's own socket, so
//! following a session costs a socket instead of a `tmux -C` process.
//!
//! The protocol is tmux's internal imsg framing (tmux 3.2 to 3.8 all speak
//! protocol version 8, but 3.6 changed the imsg header; see [`Imsg`]). A `-C`
//! client hands the server two file descriptors
//! during the handshake, and the server reads commands from one and writes
//! control-mode output to the other. We pass one end of a socketpair as both,
//! so the engine reads and writes the other end exactly as it would a
//! `tmux -C` process's stdout and stdin. The tmux socket itself only carries
//! the handshake and the server's exit and detach messages.
//!
//! The server trusts its clients: a malformed handshake message makes it call
//! `fatalx` and exit, taking every session with it. Every payload here is
//! sized exactly as tmux 3.4's `server-client.c` expects (the tests pass on
//! 3.4, 3.5a, 3.6a and 3.8), and the tests run against scratch servers only.

use std::ffi::{CString, c_int};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::task::JoinHandle;

use super::tmux::Socket;

const PROTOCOL_VERSION: u32 = 8;
const HEADER_LEN: usize = 16;
const MAX_MESSAGE: usize = 16384;
/// Before tmux 3.6: bit 0 of the 16-bit flags after the length.
const HAS_FD: u16 = 1;
/// From tmux 3.6: the top bit of the 32-bit length.
const FD_MARK: u32 = 0x8000_0000;

/// The imsg header a tmux server reads. Up to 3.5a, tmux's imsg has a 16-bit
/// length and 16 bits of flags, bit 0 saying a descriptor came with the
/// message. 3.6 took OpenBSD's newer imsg: a 32-bit length whose top bit says
/// so. A server reading the other kind drops a message that carries one, and
/// a handshake always does, so we ask tmux its version first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Imsg {
    Flags,
    Mark,
}

impl Imsg {
    /// The header for a tmux version as `tmux -V` gives it: `tmux 3.5a`,
    /// `tmux 3.6`, or `tmux next-3.7` for a build from git.
    pub fn for_version(version: &str) -> Result<Imsg> {
        let number = version.trim();
        let number = number.strip_prefix("tmux ").unwrap_or(number);
        let number = number.strip_prefix("next-").unwrap_or(number);
        let mut parts = number.splitn(2, '.');
        let major = parts.next().and_then(|m| m.parse::<u32>().ok());
        let minor = parts.next().and_then(|rest| {
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            rest[..digits].parse::<u32>().ok()
        });
        match (major, minor) {
            (Some(major), Some(minor)) if (major, minor) >= (3, 6) => Ok(Imsg::Mark),
            (Some(major), Some(minor)) if (major, minor) >= (3, 2) => Ok(Imsg::Flags),
            _ => bail!("tmux {version:?}: Parvi needs tmux 3.2 or later"),
        }
    }
}

// Client to server (tmux-protocol.h).
const IDENTIFY_FLAGS: u32 = 100;
const IDENTIFY_TERM: u32 = 101;
const IDENTIFY_TTYNAME: u32 = 102;
const IDENTIFY_STDIN: u32 = 104;
const IDENTIFY_DONE: u32 = 106;
const IDENTIFY_CLIENTPID: u32 = 107;
const IDENTIFY_CWD: u32 = 108;
const IDENTIFY_FEATURES: u32 = 109;
const IDENTIFY_STDOUT: u32 = 110;
const IDENTIFY_LONGFLAGS: u32 = 111;
const COMMAND: u32 = 200;
const EXITING: u32 = 205;
const READ_DONE: u32 = 302;
const WRITE_READY: u32 = 305;

// Server to client.
const VERSION: u32 = 12;
const DETACH: u32 = 201;
const DETACHKILL: u32 = 202;
const EXIT: u32 = 203;
const EXITED: u32 = 204;
const SHUTDOWN: u32 = 210;
const READ_OPEN: u32 = 300;
const WRITE_OPEN: u32 = 303;

/// `CLIENT_CONTROL | CLIENT_UTF8`, what `tmux -C` sends from a UTF-8 locale.
const CLIENT_FLAGS: u64 = 0x2000 | 0x10000;

/// The tmux server socket for `socket`, resolved the way tmux does.
pub fn socket_path(socket: &Socket) -> PathBuf {
    let dir = || {
        let base = std::env::var_os("TMUX_TMPDIR")
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        // SAFETY: getuid has no preconditions and cannot fail.
        base.join(format!("tmux-{}", unsafe { libc::getuid() }))
    };
    match socket {
        Socket::Default => dir().join("default"),
        Socket::Name(name) => dir().join(name),
        Socket::Path(path) => path.clone(),
    }
}

/// An attached in-process client: the control channel's two halves, and the
/// task watching the tmux socket until the server lets the client go.
pub struct Attached {
    pub reader: OwnedReadHalf,
    pub writer: OwnedWriteHalf,
    pub watcher: JoinHandle<()>,
}

/// How far a pane's output may fall behind before tmux pauses it for us. A
/// pane a second behind is better redrawn from a fresh capture than caught up.
const PAUSE_AFTER_SECONDS: u32 = 1;

/// Which imsg header the server on `socket` reads, from the version of the
/// `tmux` we run. That tmux passes descriptors with every command, so if it
/// spoke another header than the server, nothing would work. Asking the
/// server instead (`display-message -p '#{version}'`) never answered for a
/// just-launched session in our tests on tmux 3.4.
pub async fn imsg(socket: &Socket) -> Result<Imsg> {
    let output = socket
        .process()
        .arg("-V")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("running tmux")?;
    if !output.status.success() {
        bail!(
            "asking tmux its version: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Imsg::for_version(&String::from_utf8_lossy(&output.stdout))
}

/// Connects to the tmux server at `path`, which reads `imsg` headers, and
/// runs `attach-session -E -f ignore-size,pause-after=1 -t <session>` as a
/// control client.
pub async fn attach(path: &Path, imsg: Imsg, session: &str) -> Result<Attached> {
    let path = path.to_owned();
    let session = session.to_owned();
    // The handshake is a few small writes to a local socket; do them on a
    // blocking thread so fd passing can use plain sendmsg.
    let (server, ours) = tokio::task::spawn_blocking(move || handshake(&path, imsg, &session))
        .await
        .context("handshake task")??;
    server.set_nonblocking(true)?;
    ours.set_nonblocking(true)?;
    let server = UnixStream::from_std(server)?;
    let (reader, writer) = UnixStream::from_std(ours)?.into_split();
    let watcher = tokio::spawn(watch(server, imsg));
    Ok(Attached {
        reader,
        writer,
        watcher,
    })
}

/// Identifies as a control client and sends the attach command.
///
/// tmux 3.4 marks a client as control mode on IDENTIFY_FLAGS but only sets up
/// its control state on IDENTIFY_DONE. If another client detaches in between,
/// tmux writes `%client-detached` to the half-made client and crashes. So
/// everything from the flags to the command goes in one `sendmsg`, which tmux
/// reads and handles in one pass with nothing in between. Only STDIN's
/// descriptor goes ahead on its own, because a `sendmsg` carries one
/// descriptor and it must belong to the first message.
fn handshake(path: &Path, imsg: Imsg, session: &str) -> Result<(StdUnixStream, StdUnixStream)> {
    let server = StdUnixStream::connect(path)
        .with_context(|| format!("connecting to tmux at {}", path.display()))?;
    let (ours, theirs) = StdUnixStream::pair()?;
    let plain = |kind, payload: &[u8]| frame(imsg, kind, payload, false);
    send_fd(&server, imsg, IDENTIFY_STDIN, theirs.as_raw_fd(), &[])?;

    let mut rest = Vec::new();
    let flags = CLIENT_FLAGS;
    rest.extend(plain(IDENTIFY_FLAGS, &(flags as i32).to_ne_bytes())?);
    rest.extend(plain(IDENTIFY_LONGFLAGS, &flags.to_ne_bytes())?);
    let term = std::env::var("TERM").unwrap_or_default();
    rest.extend(plain(IDENTIFY_TERM, &nul_terminated(&term)?)?);
    rest.extend(plain(IDENTIFY_FEATURES, &0i32.to_ne_bytes())?);
    rest.extend(plain(IDENTIFY_TTYNAME, b"\0")?);
    let cwd = std::env::current_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_owned());
    rest.extend(plain(IDENTIFY_CWD, &nul_terminated(&cwd)?)?);
    let pid = std::process::id() as i32;
    rest.extend(plain(IDENTIFY_CLIENTPID, &pid.to_ne_bytes())?);
    // No IDENTIFY_ENVIRON: `-E` keeps attach-session from copying our
    // environment into the session, so tmux has no use for it.
    rest.extend(plain(IDENTIFY_DONE, &[])?);
    let flags = format!("ignore-size,pause-after={PAUSE_AFTER_SECONDS}");
    let args = ["attach-session", "-E", "-f", &flags, "-t", session];
    let mut command = (args.len() as i32).to_ne_bytes().to_vec();
    for arg in args {
        command.extend_from_slice(&nul_terminated(arg)?);
    }
    rest.extend(plain(COMMAND, &command)?);
    send_fd(&server, imsg, IDENTIFY_STDOUT, theirs.as_raw_fd(), &rest)?;
    drop(theirs);
    Ok((server, ours))
}

/// Watches the tmux socket. Answers what the server asks of a client and
/// returns when the server is done with us; the server closes its copies of
/// the control channel then, so the engine sees end of file.
async fn watch(mut server: UnixStream, imsg: Imsg) {
    loop {
        let Ok((kind, payload)) = receive(&mut server, imsg).await else {
            return;
        };
        let reply = match kind {
            EXIT | EXITED | SHUTDOWN | VERSION => return,
            DETACH | DETACHKILL => Some((EXITING, Vec::new())),
            // Only sent for file arguments, which we never give; refuse the
            // way tmux's own client does, with EBADF.
            READ_OPEN | WRITE_OPEN if payload.len() >= 4 => {
                let mut body = payload[..4].to_vec();
                body.extend_from_slice(&libc::EBADF.to_ne_bytes());
                let answer = if kind == READ_OPEN {
                    READ_DONE
                } else {
                    WRITE_READY
                };
                Some((answer, body))
            }
            _ => None,
        };
        if let Some((kind, body)) = reply {
            let Ok(message) = frame(imsg, kind, &body, false) else {
                return;
            };
            if tokio::io::AsyncWriteExt::write_all(&mut server, &message)
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

async fn receive(server: &mut UnixStream, imsg: Imsg) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0u8; HEADER_LEN];
    server.read_exact(&mut header).await?;
    let kind = u32::from_ne_bytes(header[0..4].try_into().unwrap());
    let len = match imsg {
        Imsg::Flags => u16::from_ne_bytes(header[4..6].try_into().unwrap()) as usize,
        Imsg::Mark => (u32::from_ne_bytes(header[4..8].try_into().unwrap()) & !FD_MARK) as usize,
    };
    if !(HEADER_LEN..=MAX_MESSAGE).contains(&len) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short imsg"));
    }
    let mut payload = vec![0u8; len - HEADER_LEN];
    server.read_exact(&mut payload).await?;
    Ok((kind, payload))
}

/// One imsg: type, total length and whether a descriptor comes with it (see
/// [`Imsg`]), protocol version as peer id, and pid -1, all in host byte
/// order, then the payload.
fn frame(imsg: Imsg, kind: u32, payload: &[u8], has_fd: bool) -> Result<Vec<u8>> {
    let len = HEADER_LEN + payload.len();
    if len > MAX_MESSAGE {
        bail!("tmux message {kind} is {len} bytes, over {MAX_MESSAGE}");
    }
    let mut message = Vec::with_capacity(len);
    message.extend_from_slice(&kind.to_ne_bytes());
    match imsg {
        Imsg::Flags => {
            message.extend_from_slice(&(len as u16).to_ne_bytes());
            message.extend_from_slice(&(if has_fd { HAS_FD } else { 0 }).to_ne_bytes());
        }
        Imsg::Mark => {
            let mark = if has_fd { FD_MARK } else { 0 };
            message.extend_from_slice(&(len as u32 | mark).to_ne_bytes());
        }
    }
    message.extend_from_slice(&PROTOCOL_VERSION.to_ne_bytes());
    message.extend_from_slice(&u32::MAX.to_ne_bytes());
    message.extend_from_slice(payload);
    Ok(message)
}

/// Sends an empty message carrying one file descriptor, followed in the same
/// `sendmsg` by `rest`, already framed. tmux wants the fd on the message that
/// starts the `sendmsg`, one fd per message.
fn send_fd(
    mut server: &StdUnixStream,
    imsg: Imsg,
    kind: u32,
    fd: RawFd,
    rest: &[u8],
) -> Result<()> {
    let mut message = frame(imsg, kind, &[], true)?;
    message.extend_from_slice(rest);
    let mut iov = libc::iovec {
        iov_base: message.as_ptr() as *mut libc::c_void,
        iov_len: message.len(),
    };
    // SAFETY: CMSG_SPACE and CMSG_LEN are pure arithmetic.
    let space = unsafe { libc::CMSG_SPACE(size_of::<c_int>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: an all-zero msghdr is valid; the pointers set below outlive the
    // sendmsg call, and the control buffer has room for one fd.
    let sent = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = space as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<c_int>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg) as *mut c_int, fd);
        libc::sendmsg(server.as_raw_fd(), &msg, libc::MSG_NOSIGNAL)
    };
    if sent < 0 {
        return Err(io::Error::last_os_error()).context("passing a descriptor to tmux");
    }
    // The fd went with the first byte; a short write only delays the rest.
    server.write_all(&message[sent as usize..])?;
    Ok(())
}

/// A string as tmux wants it: its bytes and a NUL, with no NUL inside.
fn nul_terminated(value: &str) -> Result<Vec<u8>> {
    Ok(CString::new(value)
        .context("string with a NUL inside")?
        .into_bytes_with_nul())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_tmuxs_header() {
        let message = frame(Imsg::Flags, COMMAND, b"ab", false).unwrap();
        assert_eq!(message.len(), 18);
        assert_eq!(&message[0..4], &200u32.to_ne_bytes());
        assert_eq!(&message[4..6], &18u16.to_ne_bytes());
        assert_eq!(&message[6..8], &0u16.to_ne_bytes());
        assert_eq!(&message[8..12], &8u32.to_ne_bytes());
        assert_eq!(&message[12..16], &u32::MAX.to_ne_bytes());
        assert_eq!(&message[16..], b"ab");
        let with_fd = frame(Imsg::Flags, IDENTIFY_STDIN, &[], true).unwrap();
        assert_eq!(&with_fd[6..8], &1u16.to_ne_bytes());

        // From tmux 3.6 the length is 32 bits, its top bit marking a
        // descriptor.
        let message = frame(Imsg::Mark, COMMAND, b"ab", false).unwrap();
        assert_eq!(&message[4..8], &18u32.to_ne_bytes());
        assert_eq!(
            &message[8..],
            &frame(Imsg::Flags, COMMAND, b"ab", false).unwrap()[8..]
        );
        let with_fd = frame(Imsg::Mark, IDENTIFY_STDIN, &[], true).unwrap();
        assert_eq!(&with_fd[4..8], &(16u32 | 0x8000_0000).to_ne_bytes());
    }

    #[test]
    fn picks_the_header_by_version() {
        for (version, imsg) in [
            ("tmux 3.4\n", Imsg::Flags),
            ("tmux next-3.7", Imsg::Mark),
            ("3.2", Imsg::Flags),
            ("3.4", Imsg::Flags),
            ("3.5a", Imsg::Flags),
            ("3.6", Imsg::Mark),
            ("3.6a\n", Imsg::Mark),
            ("3.8", Imsg::Mark),
            ("next-3.7", Imsg::Mark),
            ("4.0", Imsg::Mark),
        ] {
            assert_eq!(Imsg::for_version(version).unwrap(), imsg, "{version}");
        }
        for version in ["3.1c", "2.9", "master", "openbsd-7.6", ""] {
            assert!(Imsg::for_version(version).is_err(), "{version:?}");
        }
    }

    #[test]
    fn refuses_oversized_messages() {
        assert!(frame(Imsg::Mark, COMMAND, &vec![b'x'; MAX_MESSAGE], false).is_err());
        let largest = vec![b'x'; MAX_MESSAGE - HEADER_LEN];
        assert!(frame(Imsg::Mark, COMMAND, &largest, false).is_ok());
    }

    #[test]
    fn strings_end_in_nul_and_refuse_inner_nuls() {
        assert_eq!(nul_terminated("ab").unwrap(), b"ab\0");
        assert!(nul_terminated("a\0b").is_err());
    }

    #[test]
    fn resolves_socket_paths_like_tmux() {
        let path = socket_path(&Socket::Path("/x/y".into()));
        assert_eq!(path, PathBuf::from("/x/y"));
        let named = socket_path(&Socket::Name("work".into()));
        assert!(named.ends_with("work"));
        assert!(named.parent().unwrap().to_string_lossy().contains("tmux-"));
    }
}
