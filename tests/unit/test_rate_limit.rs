#[cfg(test)]
mod tests {
    use ferrum::middleware::rate_limit::RateLimiter;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_rate_limiter_sliding_window() {
        // Limit: 3 requests per 1 second
        let limiter = RateLimiter::new(3, 1);

        assert!(limiter.check_limit("user-1"));
        assert!(limiter.check_limit("user-1"));
        assert!(limiter.check_limit("user-1"));
        
        // 4th request within 1s should be rate limited
        assert!(!limiter.check_limit("user-1"));

        // Different user should be allowed
        assert!(limiter.check_limit("user-2"));

        // Wait for window to slide past (1.1 seconds)
        thread::sleep(Duration::from_millis(1100));

        // Should be allowed again
        assert!(limiter.check_limit("user-1"));
        assert!(limiter.check_limit("user-1"));
        assert!(limiter.check_limit("user-1"));
        assert!(!limiter.check_limit("user-1"));
    }

    #[test]
    fn test_rate_limiter_cleanup() {
        let limiter = RateLimiter::new(2, 1);
        
        assert!(limiter.check_limit("temp-user"));
        limiter.cleanup();
        // Since window is active, it shouldn't clean up the key yet
        
        thread::sleep(Duration::from_millis(1100));
        limiter.cleanup();
        // Now it should clean up the key because timestamps are expired and vector is empty
        // We test that check_limit still works correctly after cleanup
        assert!(limiter.check_limit("temp-user"));
    }
}
