use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_connections: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
    pub header_style: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub window_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackConfig {
    pub enabled: bool,
    pub default_fallback_model: String,
    pub max_attempts: u32,
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub providers: HashMap<String, ProviderConfig>,
    pub rate_limit: RateLimitConfig,
    pub metrics: MetricsConfig,
    pub fallback: Option<FallbackConfig>,
}

impl AppConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = fs::read_to_string(path)?;
        let mut config: AppConfig = toml::from_str(&content)?;

        // Override server port if FERRUM_PORT environment variable is set
        if let Ok(port_str) = env::var("FERRUM_PORT") {
            if let Ok(port) = port_str.parse::<u16>() {
                config.server.port = port;
            }
        }

        // Override server host if FERRUM_HOST environment variable is set
        if let Ok(host) = env::var("FERRUM_HOST") {
            config.server.host = host;
        }

        Ok(config)
    }
}
