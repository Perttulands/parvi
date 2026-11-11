pub(crate) fn io(error: std::io::Error) -> parvi_core::Error {
    use std::io::ErrorKind;
    let code = match error.kind() {
        ErrorKind::NotFound => "not_found",
        ErrorKind::AlreadyExists => "conflict",
        ErrorKind::InvalidInput => "invalid_input",
        ErrorKind::InvalidData => "invalid_data",
        ErrorKind::PermissionDenied => "permission_denied",
        ErrorKind::Interrupted => "interrupted",
        ErrorKind::TimedOut => "timed_out",
        _ => "io",
    };
    parvi_core::Error::new(code, error.to_string())
}

pub(crate) fn to_io(error: parvi_core::Error) -> std::io::Error {
    use std::io::ErrorKind;
    let kind = match error.code.as_str() {
        "not_found" => ErrorKind::NotFound,
        "conflict" => ErrorKind::AlreadyExists,
        "invalid_input" => ErrorKind::InvalidInput,
        "invalid_data" => ErrorKind::InvalidData,
        "permission_denied" => ErrorKind::PermissionDenied,
        "interrupted" => ErrorKind::Interrupted,
        "timed_out" => ErrorKind::TimedOut,
        _ => ErrorKind::Other,
    };
    std::io::Error::new(kind, error.message)
}
