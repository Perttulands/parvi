//! Reading definition files that may be symlinks, for example a mission kept
//! in a repository. A link is followed; one whose target cannot be opened is
//! named as a broken link instead of a missing file.

use std::io::ErrorKind;
use std::path::Path;

use crate::error::DefinitionError;

/// Reads a definition file through any symlink. Only a regular file is read,
/// since a FIFO or a device would block or never end.
pub fn read_definition(path: &Path) -> Result<String, DefinitionError> {
    let shown = path.display();
    let link = match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(DefinitionError::NotFound(shown.to_string()));
        }
        Err(err) => return Err(DefinitionError::Io(format!("{shown}: {err}"))),
        Ok(metadata) => metadata,
    };
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if link.file_type().is_symlink() => return Err(broken_link(path, &err)),
        Err(err) => return Err(DefinitionError::Io(format!("{shown}: {err}"))),
    };
    if !metadata.is_file() {
        return Err(DefinitionError::Io(format!(
            "{shown} is not a regular file"
        )));
    }
    let bytes =
        std::fs::read(path).map_err(|err| DefinitionError::Io(format!("{shown}: {err}")))?;
    String::from_utf8(bytes)
        .map_err(|_| DefinitionError::InvalidSource(format!("{shown} is not UTF-8")))
}

fn broken_link(path: &Path, err: &std::io::Error) -> DefinitionError {
    let target =
        std::fs::read_link(path).map_or_else(|_| "?".to_owned(), |t| t.display().to_string());
    let problem = match err.kind() {
        ErrorKind::NotFound => "does not exist".to_owned(),
        ErrorKind::PermissionDenied => "Parvi's user may not reach".to_owned(),
        _ => format!("cannot be reached: {err}"),
    };
    DefinitionError::BrokenLink(format!(
        "{} is a symlink to {target}, which {problem}",
        path.display()
    ))
}

/// The names in a directory that end with `suffix`, without it, sorted. A
/// missing directory holds none.
pub fn list_names(dir: &Path, suffix: &str) -> Result<Vec<String>, DefinitionError> {
    let entries = match std::fs::read_dir(dir) {
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(DefinitionError::Io(format!("{}: {err}", dir.display()))),
        Ok(entries) => entries,
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| DefinitionError::Io(format!("{}: {err}", dir.display())))?;
        if let Some(name) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.strip_suffix(suffix))
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}
