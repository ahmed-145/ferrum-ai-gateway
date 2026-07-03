#[cfg(test)]
mod tests {
    use ferrum::config::AppConfig;
    use std::env;
    use std::fs;

    #[test]
    fn test_load_config() {
        let temp_toml = r#"
            [server]
            host = "127.0.0.1"
            port = 9000
            max_connections = 5000

            [providers.openai]
            base_url = "https://api.openai.com/v1"
            timeout_secs = 10
            max_retries = 3

            [rate_limit]
            requests_per_minute = 100
            window_secs = 60

            [metrics]
            enabled = true
            path = "/metrics"
        "#;

        let temp_path = "test_ferrum_temp.toml";
        fs::write(temp_path, temp_toml).unwrap();

        // Test normal load
        let config = AppConfig::load(temp_path).unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.max_connections, Some(5000));
        assert_eq!(config.providers.get("openai").unwrap().base_url, "https://api.openai.com/v1");
        assert_eq!(config.rate_limit.requests_per_minute, 100);
        assert!(config.metrics.enabled);
        assert_eq!(config.metrics.path, "/metrics");

        // Test environment override
        env::set_var("FERRUM_PORT", "1234");
        env::set_var("FERRUM_HOST", "0.0.0.0");
        let config_override = AppConfig::load(temp_path).unwrap();
        assert_eq!(config_override.server.port, 1234);
        assert_eq!(config_override.server.host, "0.0.0.0");

        env::remove_var("FERRUM_PORT");
        env::remove_var("FERRUM_HOST");
        fs::remove_file(temp_path).unwrap();
    }
}
