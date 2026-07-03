use axum::{
    body::Body,
    routing::post,
    response::Response,
    Router,
};
use bytes::Bytes;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

use ferrum::config::{AppConfig, ProviderConfig, ServerConfig, RateLimitConfig, MetricsConfig};
use ferrum::middleware::metrics::MetricsCollector;
use ferrum::middleware::rate_limit::RateLimiter;
use ferrum::router::create_router;
use ferrum::AppState;

// Helper to spawn a mock server returning custom responses
async fn spawn_mock_server<F, Fut>(handler: F) -> SocketAddr
where
    F: Fn(Body) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Response> + Send,
{
    let app = Router::new().route(
        "/*path",
        post(move |axum::extract::Path(_path): axum::extract::Path<String>, body: Body| {
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
async fn test_openai_compatible_stream_passthrough() {
    // 1. Spawn a mock provider server that streams OpenAI-compatible chunks
    let mock_provider_addr = spawn_mock_server(|_body| async {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("data: [DONE]\n\n")),
        ]);
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    })
    .await;

    // 2. Configure Ferrum to point to this mock provider
    let mut providers = HashMap::new();
    providers.insert(
        "groq".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", mock_provider_addr),
            timeout_secs: 5,
            max_retries: 1,
            header_style: None,
        },
    );

    let config = AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // dynamic
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
        fallback: None,
    };

    let client = reqwest::Client::new();
    let state = Arc::new(AppState {
        config,
        client,
        rate_limiter: RateLimiter::new(100, 60),
        metrics: MetricsCollector::new(),
    });

    // 3. Spawn local Ferrum gateway
    let ferrum_router = create_router(state);
    let ferrum_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ferrum_addr = ferrum_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(ferrum_listener, ferrum_router).await.unwrap();
    });

    // 4. Send client request
    let client = reqwest::Client::new();
    let req_body = serde_json::json!({
        "model": "groq/llama-3.1-8b-instant",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    let res = client
        .post(format!("http://{}/v1/chat/completions", ferrum_addr))
        .header("Authorization", "Bearer mock-key")
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(res.headers().get("content-type").unwrap(), "text/event-stream");

    // 5. Assert streamed content is correct
    let mut stream = res.bytes_stream();
    let mut received = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.unwrap();
        received.push_str(std::str::from_utf8(&bytes).unwrap());
    }

    assert!(received.contains("Hello"));
    assert!(received.contains("world"));
    assert!(received.contains("[DONE]"));
}

#[tokio::test]
async fn test_anthropic_stream_translation() {
    // 1. Spawn a mock Anthropic provider server that streams Anthropic SSE events
    let mock_provider_addr = spawn_mock_server(|_body| async {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-1\",\"model\":\"claude-3-5-sonnet-20240620\"}}\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" Anthropic\"}}\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n")),
            Ok::<_, std::io::Error>(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")),
        ]);
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    })
    .await;

    // 2. Configure Ferrum pointing to Anthropic mock
    let mut providers = HashMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", mock_provider_addr),
            timeout_secs: 5,
            max_retries: 1,
            header_style: Some("anthropic".to_string()),
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
        fallback: None,
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

    // 3. Client calls OpenAI completions route with Anthropic model prefix
    let client = reqwest::Client::new();
    let req_body = serde_json::json!({
        "model": "anthropic/claude-3-5-sonnet",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    let res = client
        .post(format!("http://{}/v1/chat/completions", ferrum_addr))
        .header("Authorization", "Bearer mock-key")
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    // 4. Assert stream is translated to OpenAI format
    let mut stream = res.bytes_stream();
    let mut received = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.unwrap();
        received.push_str(std::str::from_utf8(&bytes).unwrap());
    }

    // Verify it contains standard OpenAI chat completions fields
    assert!(received.contains("chat.completion.chunk"));
    assert!(received.contains("Hello"));
    assert!(received.contains("Anthropic"));
    assert!(received.contains("msg-1"));
    assert!(received.contains("[DONE]"));
}
