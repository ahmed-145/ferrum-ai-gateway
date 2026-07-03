pub mod config;
pub mod health;
pub mod middleware;
pub mod providers;
pub mod proxy;
pub mod router;

use config::AppConfig;
use middleware::metrics::MetricsCollector;
use middleware::rate_limit::RateLimiter;
use reqwest::Client;

pub struct AppState {
    pub config: AppConfig,
    pub client: Client,
    pub rate_limiter: RateLimiter,
    pub metrics: MetricsCollector,
}
