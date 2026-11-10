//! Why the Brain could not answer, as an HTTP status and a sentence.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use parvi_brain_contract::{
    BAD_REQUEST, BRAIN_ERROR, CONFLICT, FORBIDDEN, NOT_FOUND, TOO_LARGE, UNAVAILABLE,
};

#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub status: StatusCode,
    pub message: String,
}

impl Refusal {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Refusal {
        Refusal {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let code = code(self.status);
        parvi_node_kit::Failure(self.status, parvi_core::Error::new(code, self.message))
            .into_response()
    }
}

pub fn code(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => BAD_REQUEST,
        403 => FORBIDDEN,
        404 => NOT_FOUND,
        409 => CONFLICT,
        413 => TOO_LARGE,
        503 => UNAVAILABLE,
        _ => BRAIN_ERROR,
    }
}
