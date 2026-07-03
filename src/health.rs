use axum::{http::StatusCode, response::IntoResponse};

pub async fn liveness() -> impl IntoResponse {
    StatusCode::OK
}

pub async fn readiness() -> impl IntoResponse {
    // If we had a database or Redis, we would check connection health here.
    // For now, it's a simple readiness check.
    StatusCode::OK
}
