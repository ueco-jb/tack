use axum::http::header;
use axum::response::IntoResponse;

pub async fn index() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'"),
        ],
        include_str!("../web/index.html"),
    )
}
