use std::env;

#[tokio::test]
async fn test_live_groq_completions_if_key_available() {
    let key = match env::var("GROQ_API_KEY") {
        Ok(k) if !k.is_empty() && k != "your_groq_key_here" => k,
        _ => {
            println!("Skipping live Groq integration test: GROQ_API_KEY environment variable not set.");
            return;
        }
    };

    println!("Running live Groq integration test...");
    // Let's spin up our gateway with default configuration and test against it!
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use ferrum::config::{AppConfig, ProviderConfig, ServerConfig, RateLimitConfig, MetricsConfig};
    use ferrum::middleware::metrics::MetricsCollector;
    use ferrum::middleware::rate_limit::RateLimiter;
    use ferrum::router::create_router;
    use ferrum::AppState;

    let mut providers = HashMap::new();
    providers.insert(
        "groq".to_string(),
        ProviderConfig {
            api_key: Some(key),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            timeout_secs: 30,
            max_retries: 3,
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

    // Send chat completions request
    let client = reqwest::Client::new();
    let req_body = serde_json::json!({
        "model": "groq/llama-3.1-8b-instant",
        "messages": [{"role": "user", "content": "Say hello in one word."}]
    });

    let res = client
        .post(format!("http://{}/v1/chat/completions", ferrum_addr))
        .header("Authorization", "Bearer test-client-key")
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let res_json: serde_json::Value = res.json().await.unwrap();
    let content = res_json["choices"][0]["message"]["content"].as_str().unwrap();
    println!("Response content: {}", content);
    assert!(!content.is_empty());
}
