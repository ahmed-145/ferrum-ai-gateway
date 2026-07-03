use axum::{
    extract::State,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use crate::AppState;
use crate::health::{liveness, readiness};
use crate::proxy::{handle_chat_completions, handle_messages};

async fn metrics_handler(State(app_state): State<Arc<AppState>>) -> String {
    app_state.metrics.render()
}

pub fn create_router(app_state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/messages", post(handle_messages))
        .route("/health/liveness", get(liveness))
        .route("/health/readiness", get(readiness))
        .route("/metrics", get(metrics_handler))
        .layer(cors)
        .with_state(app_state)
}
