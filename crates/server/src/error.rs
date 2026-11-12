use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tack_core::api::ErrorBody;
use tack_core::{Error, ErrorKind};

#[derive(Debug)]
pub struct ApiError(pub Error);

pub fn status_of(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.0.kind == ErrorKind::Internal {
            eprintln!("tack: internal error: {}", self.0.message);
        }
        (status_of(self.0.kind), Json(ErrorBody::from(&self.0))).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        Self(Error::invalid(format!(
            "bad request body: {}",
            r.body_text()
        )))
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        Self(Error::invalid(format!(
            "bad query string: {}",
            r.body_text()
        )))
    }
}

impl From<PathRejection> for ApiError {
    fn from(r: PathRejection) -> Self {
        Self(Error::invalid(format!("bad path: {}", r.body_text())))
    }
}
