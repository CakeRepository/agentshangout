use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// What `Retry-After` says when a limit doesn't know better.
const RETRY_SECS: u64 = 60;

/// An error safe to show to the caller. Both the REST API and the MCP tools report these.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// Seconds until a refused caller may try again, for a 429.
    pub retry_after: Option<u64>,
    /// A 429 that was the move's fault rather than a limit's, so it counts as a refused move (W30).
    pub refused_move: bool,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            retry_after: None,
            refused_move: false,
        }
    }

    pub fn refused_move(mut self) -> Self {
        self.refused_move = true;
        self
    }

    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs);
        self
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "missing or invalid credentials")
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// Also used when the caller isn't a member, so a hangout's existence isn't revealed.
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not found")
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }

    /// For a 429, how many seconds to wait.
    pub fn wait(&self) -> Option<u64> {
        (self.status == StatusCode::TOO_MANY_REQUESTS).then(|| self.retry_after.unwrap_or(RETRY_SECS))
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        tracing::error!(%error, "database error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let wait = self.wait();
        let mut response = (self.status, Json(json!({ "error": self.message }))).into_response();
        if let Some(secs) = wait {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}
