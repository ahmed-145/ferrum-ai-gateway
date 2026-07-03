use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub struct Histogram {
    pub count: AtomicU64,
    pub sum_micros: AtomicU64,
    pub buckets: [AtomicU64; 12],
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            sum_micros: AtomicU64::new(0),
            buckets: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    pub fn record(&self, duration: Duration) {
        let secs = duration.as_secs_f64();
        let micros = duration.as_micros() as u64;

        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);

        let bucket_thresholds = [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];
        for (i, &threshold) in bucket_thresholds.iter().enumerate() {
            if secs <= threshold {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

pub struct MetricsCollector {
    // Key: provider="...",model="...",route="...",status="..."
    pub request_counters: DashMap<String, AtomicU64>,
    // Key: provider="...",model="...",route="..."
    pub latency_histograms: DashMap<String, Histogram>,
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            request_counters: DashMap::new(),
            latency_histograms: DashMap::new(),
        }
    }

    pub fn record_request(&self, provider: &str, model: &str, route: &str, status: u16, duration: Duration) {
        let counter_key = format!(
            "provider=\"{}\",model=\"{}\",route=\"{}\",status=\"{}\"",
            provider, model, route, status
        );
        let counter_entry = self.request_counters.entry(counter_key).or_insert_with(|| AtomicU64::new(0));
        counter_entry.value().fetch_add(1, Ordering::Relaxed);

        let hist_key = format!(
            "provider=\"{}\",model=\"{}\",route=\"{}\"",
            provider, model, route
        );
        let hist_entry = self.latency_histograms.entry(hist_key).or_insert_with(Histogram::new);
        hist_entry.value().record(duration);
    }

    pub fn render(&self) -> String {
        let mut out = String::new();

        // Render request counters
        out.push_str("# HELP ferrum_requests_total Total number of API requests routed through Ferrum\n");
        out.push_str("# TYPE ferrum_requests_total counter\n");
        for r in self.request_counters.iter() {
            let labels = r.key();
            let count = r.value().load(Ordering::Relaxed);
            out.push_str(&format!("ferrum_requests_total{{{}}} {}\n", labels, count));
        }

        // Render latency histograms
        out.push_str("# HELP ferrum_request_duration_seconds Latency of API requests in seconds\n");
        out.push_str("# TYPE ferrum_request_duration_seconds histogram\n");

        let bucket_thresholds = [
            "0.001", "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1.0", "2.5", "5.0", "10.0",
        ];
        for r in self.latency_histograms.iter() {
            let labels = r.key();
            let hist = r.value();
            let count = hist.count.load(Ordering::Relaxed);
            let sum = hist.sum_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;

            for (i, &le) in bucket_thresholds.iter().enumerate() {
                let val = hist.buckets[i].load(Ordering::Relaxed);
                out.push_str(&format!(
                    "ferrum_request_duration_seconds_bucket{{{},le=\"{}\"}} {}\n",
                    labels, le, val
                ));
            }
            // Add le="+Inf" bucket
            out.push_str(&format!(
                "ferrum_request_duration_seconds_bucket{{{},le=\"+Inf\"}} {}\n",
                labels, count
            ));
            out.push_str(&format!("ferrum_request_duration_seconds_sum{{{}}} {}\n", labels, sum));
            out.push_str(&format!("ferrum_request_duration_seconds_count{{{}}} {}\n", labels, count));
        }

        out
    }
}
