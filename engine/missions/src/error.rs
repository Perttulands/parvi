pub(crate) fn io(error: std::io::Error) -> parvi_core::Error {
    parvi_core::Error::new("io", error.to_string())
}
