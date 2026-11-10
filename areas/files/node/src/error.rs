//! What can go wrong with a file, in words the operator reads.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum FilesError {
    /// The path is not under any of this node's roots, or is not absolute.
    Outside,
    /// A root itself cannot be created, renamed or deleted.
    Root,
    NotFound,
    /// Creating or renaming onto something already there.
    Exists,
    /// A save from a version the file no longer is: it changed since it was
    /// read.
    Changed,
    Permission,
    /// A folder was wanted.
    NotFolder,
    /// A file was wanted.
    NotFile,
    /// Over the most this node reads or writes in one request.
    TooLarge,
    /// The request names another node; reaching it is the serving node's pipe.
    OtherNode(String),
    Io(io::Error),
}

impl From<io::Error> for FilesError {
    fn from(error: io::Error) -> FilesError {
        match error.kind() {
            io::ErrorKind::NotFound => FilesError::NotFound,
            io::ErrorKind::PermissionDenied => FilesError::Permission,
            io::ErrorKind::AlreadyExists => FilesError::Exists,
            io::ErrorKind::NotADirectory => FilesError::NotFolder,
            io::ErrorKind::IsADirectory => FilesError::NotFile,
            _ => FilesError::Io(error),
        }
    }
}

impl std::error::Error for FilesError {}

impl fmt::Display for FilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilesError::Outside => write!(f, "Outside this node's roots"),
            FilesError::Root => write!(f, "A root cannot be changed"),
            FilesError::NotFound => write!(f, "Not found"),
            FilesError::Exists => write!(f, "Something already has that name"),
            FilesError::Changed => write!(f, "Changed on the device since it was read"),
            FilesError::Permission => write!(f, "Permission denied"),
            FilesError::NotFolder => write!(f, "Not a folder"),
            FilesError::NotFile => write!(f, "Not a file"),
            FilesError::TooLarge => write!(f, "Too large"),
            FilesError::OtherNode(node) => write!(f, "Node {node} is not reachable here"),
            FilesError::Io(error) => write!(f, "{error}"),
        }
    }
}
