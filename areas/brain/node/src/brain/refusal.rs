//! Why the Brain could not answer, as an HTTP status and a sentence.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

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
        let body = Json(parvi_core::Error::new(code, self.message));
        (self.status, body).into_response()
    }
}

pub fn code(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        413 => "too_large",
        503 => "unavailable",
        _ => "brain_error",
    }
}
