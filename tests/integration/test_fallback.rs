use axum::{
    body::Body,
    routing::post,
    Router,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

use ferrum::config::{AppConfig, ProviderConfig, ServerConfig, RateLimitConfig, MetricsConfig, FallbackConfig};
use ferrum::middleware::metrics::MetricsCollector;
use ferrum::middleware::rate_limit::RateLimiter;
use ferrum::router::create_router;
use ferrum::AppState;

async fn spawn_mock_server<F, Fut>(handler: F) -> SocketAddr
where
    F: Fn(Body) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = axum::response::Response> + Send,
{
    let app = Router::new().route(
        "/*path",
        post(move |body: Body| {
            let handler = handler.clone();
            async move { handler(body).await }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn test_provider_failover_routing() {
    use axum::response::Response;
    use std::sync::atomic::{AtomicU32, Ordering};

    let primary_calls = Arc::new(AtomicU32::new(0));
    let fallback_calls = Arc::new(AtomicU32::new(0));

    // 1. Spawn primary mock server (always returns 502 Bad Gateway)
    let primary_calls_clone = Arc::clone(&primary_calls);
    let primary_addr = spawn_mock_server(move |_body| {
        let primary_calls_clone = Arc::clone(&primary_calls_clone);
        async move {
            primary_calls_clone.fetch_add(1, Ordering::Relaxed);
            Response::builder()
                .status(502)
                .body(Body::from("{\"error\": \"Primary failed\"}"))
                .unwrap()
        }
    })
    .await;

    // 2. Spawn fallback mock server (returns 200 OK success)
    let fallback_calls_clone = Arc::clone(&fallback_calls);
    let fallback_addr = spawn_mock_server(move |_body| {
        let fallback_calls_clone = Arc::clone(&fallback_calls_clone);
        async move {
            fallback_calls_clone.fetch_add(1, Ordering::Relaxed);
            Response::builder()
                .status(200)
                .body(Body::from("{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"Fallback success\"}}]}"))
                .unwrap()
        }
    })
    .await;

    // 3. Setup Config with fallback model mapping
    let mut providers = HashMap::new();
    providers.insert(
        "groq".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", primary_addr),
            timeout_secs: 5,
            max_retries: 0, // no retries on primary directly to failover quickly
            header_style: None,
        },
    );
    providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", fallback_addr),
            timeout_secs: 5,
            max_retries: 1,
            header_style: None,
        },
    );

    let config = AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            max_connections: Some(100),
        },
        providers,
        rate_limit: RateLimitConfig {
            requests_per_minute: 100,
            window_secs: 60,
        },
        metrics: MetricsConfig {
            enabled: true,
            path: "/metrics".to_string(),
        },
        fallback: Some(FallbackConfig {
            enabled: true,
            default_fallback_model: "openai/gpt-4o".to_string(),
            max_attempts: 1,
            backoff_ms: 10,
        }),
    };

    let client = reqwest::Client::new();
    let state = Arc::new(AppState {
        config,
        client,
        rate_limiter: RateLimiter::new(100, 60),
        metrics: MetricsCollector::new(),
    });

    let ferrum_router = create_router(state);
    let ferrum_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ferrum_addr = ferrum_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(ferrum_listener, ferrum_router).await.unwrap();
    });

    // 4. Call primary model groq/llama-3.1-8b-instant
    let client = reqwest::Client::new();
    let req_body = serde_json::json!({
        "model": "groq/llama-3.1-8b-instant",
        "messages": [{"role": "user", "content": "hi"}]
    });

    let res = client
        .post(format!("http://{}/v1/chat/completions", ferrum_addr))
        .header("Authorization", "Bearer mock-key")
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let res_json: serde_json::Value = res.json().await.unwrap();
    assert_eq!(res_json["choices"][0]["message"]["content"].as_str().unwrap(), "Fallback success");

    // Primary should have been called, failed, then fallback called
    assert_eq!(primary_calls.load(Ordering::Relaxed), 1);
    assert_eq!(fallback_calls.load(Ordering::Relaxed), 1);
}
