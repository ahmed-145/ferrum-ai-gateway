use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use ferrum::config::AppConfig;
use ferrum::middleware::metrics::MetricsCollector;
use ferrum::middleware::rate_limit::RateLimiter;
use ferrum::router::create_router;
use ferrum::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Initialize logging
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(std::env::var("FERRUM_LOG_LEVEL").unwrap_or_else(|_| "info".to_string())));
    
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    info!("Starting Ferrum — High-Performance Rust AI Gateway");

    // 2. Load configuration
    let config_path = std::env::var("FERRUM_CONFIG_PATH").unwrap_or_else(|_| "ferrum.toml".to_string());
    let config = AppConfig::load(&config_path).unwrap_or_else(|e| {
        warn!("Failed to load config from {}: {}. Using default values.", config_path, e);
        // Fallback or exit
        panic!("Configuration loading failed: {}", e);
    });

    // 3. Create HTTP client pool with connection pooling and keep-alive
    let max_conns = config.server.max_connections.unwrap_or(10000);
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(max_conns)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Some(Duration::from_secs(60)))
        .build()?;

    // 4. Initialize shared state
    let rate_limiter = RateLimiter::new(config.rate_limit.requests_per_minute, config.rate_limit.window_secs);
    let metrics = MetricsCollector::new();

    let app_state = Arc::new(AppState {
        config: config.clone(),
        client,
        rate_limiter,
        metrics,
    });

    // 5. Spawn background rate-limiter cleanup task
    let cleanup_state = Arc::clone(&app_state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            cleanup_state.rate_limiter.cleanup();
        }
    });

    // 6. Build Axum router
    let app = create_router(Arc::clone(&app_state));

    // 7. Start TCP listener
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Server listening on http://{}", addr);

    // 8. Run Axum server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Ferrum gateway stopped.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received. Initiating graceful teardown...");
}
