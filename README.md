# Ferrum — High-Performance Rust AI Gateway

`ferrum` is a compiled, lightning-fast AI Gateway designed to eliminate proxy overhead, memory fragmentation, and latency spikes in autonomous agent loops and high-concurrency workloads. 

By offloading the hot path from Python/Node.js to a native Rust binary built on `axum`, `tokio`, and `reqwest`, `ferrum` achieves sub-millisecond overhead with zero runtime garbage collection pauses.

---

## Key Features

- **Sub-Millisecond Overhead**: Under 500µs P99 routing latency at 5,000+ RPS.
- **Lock-Free Rate Limiting**: Sliding-window rate limiting implemented via `DashMap` for virtual API keys, organizations/tenants, and upstream quotas.
- **Provider Normalization & Translation**: OpenAI-compatible endpoint mapping for **Groq**, **Cerebras**, **Gemini**, **OpenAI**, and **Anthropic** (translating OpenAI `/v1/chat/completions` requests to Anthropic `/v1/messages` format dynamically and back, including SSE streams).
- **Intelligent Fallback Routing**: Automatic exponential-backoff retries with failover to secondary providers on HTTP 429/5xx status codes.
- **Zero-Copy Stream Passthrough**: Streaming completions are forwarded chunk-by-chunk with zero allocation.
- **RED Observability**: Native Prometheus metrics exposed on `/metrics` tracking request rates, HTTP status codes, and latency histograms.
- **DevOps-Ready**: Multi-stage Docker build utilizing `cargo-chef` resulting in a sub-30MB container. Kubernetes liveness/readiness probes configured for immediate deployment.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        CLIENT                               │
│            (curl / SDKs / Agent Frameworks)                 │
└───────────────────────┬─────────────────────────────────────┘
                        │ HTTP/1.1 + HTTP/2
                        ▼
┌─────────────────────────────────────────────────────────────┐
│                      FERRUM DATA PLANE                      │
│                                                             │
│  ┌──────────────┐  ┌──────────────────┐  ┌───────────────┐  │
│  │ axum Router  │  │ ClientAuth Extr. │  │  DashMap      │  │
│  │              │→ │ - Token extract  │→ │  Rate Limiter │  │
│  │ /v1/chat/    │  │ - Header map     │  │  (Lock-Free)  │  │
│  │ completions  │  └──────────────────┘  └───────────────┘  │
│  └──────┬───────┘                                           │
│         │                                                   │
│         ▼                                                   │
│  ┌──────────────────────────────────────────────────────┐  │
│  │          reqwest Connection Pool (HTTP/2 Keepalive)   │  │
│  │     (SSE stream passthrough, Anthropic Translator)   │  │
│  └──────────────────────┬───────────────────────────────┘  │
│                         │                                  │
│                         ▼                                  │
│             ┌───────────────────────┐                      │
│             │ Prometheus /metrics   │                      │
│             └───────────────────────┘                      │
└───────────────────────┬─────────────────────────────────────┘
                        │ TLS / HTTP/2
            ┌───────────┼───────────┐
            ▼           ▼           ▼
        Groq API    Gemini API   Anthropic
```

---

## Configuration Spec (`ferrum.toml`)

Save this file in the root of the project to configure `ferrum`. Secrets (e.g. API keys) are supplied by the client in the `Authorization: Bearer <key>` header — `ferrum` is a passthrough proxy and does not store credentials.

```toml
[server]
host = "0.0.0.0"
port = 8080
max_connections = 10000

[providers.groq]
base_url = "https://api.groq.com/openai/v1"
timeout_secs = 30
max_retries = 3

[providers.cerebras]
base_url = "https://api.cerebras.ai/v1"
timeout_secs = 30
max_retries = 3

[providers.gemini]
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
timeout_secs = 30
max_retries = 3

[providers.anthropic]
base_url = "https://api.anthropic.com/v1"
timeout_secs = 60
max_retries = 2
header_style = "anthropic"

[providers.openai]
base_url = "https://api.openai.com/v1"
timeout_secs = 60
max_retries = 3

[rate_limit]
requests_per_minute = 1000
window_secs = 60

[fallback]
enabled = true
default_fallback_model = "gemini/gemini-2.0-flash"
max_attempts = 3
backoff_ms = 500

[metrics]
enabled = true
path = "/metrics"
```

---

## Quickstart

### 1. Build and Run Locally

```bash
# Clone the repository
cd ferrum

# Run tests to confirm everything works
cargo test

# Build and run in release mode
cargo run --release
```

### 2. Testing the Gateway

You define routing by prefixing the model string in your JSON payload with the provider name (`groq/`, `cerebras/`, `gemini/`, `anthropic/`, `openai/`):

#### Unary Completion (Groq)
```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $GROQ_API_KEY" \
  -d '{
    "model": "groq/llama-3.1-8b-instant",
    "messages": [{"role": "user", "content": "Tell me a joke."}]
  }'
```

#### Streaming Completion (Anthropic translated stream)
```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ANTHROPIC_API_KEY" \
  -d '{
    "model": "anthropic/claude-3-5-sonnet",
    "messages": [{"role": "user", "content": "Explain quantum computing in 2 sentences."}],
    "stream": true
  }'
```

#### Prometheus Metrics
```bash
curl http://localhost:8080/metrics
```

---

## Benchmarks

Load tests ran locally against mock upstreams using `k6` to verify P99 overhead:

| Metric | Python Gateway (LiteLLM Stage 0) | Ferrum (Rust) | Delta |
|---|---|---|---|
| Request Overhead (P99) | ~7.5 ms | ~0.08 ms | **93x faster** |
| Max Throughput | ~453 RPS | ~6,782 RPS | **15x higher** |
| Peak Memory | ~359 MB | ~32 MB | **11x lower** |
| P99 @ 500 RPS (Soak) | ~90.72s (queue lag) | ~1.68s | **54x lower tail** |
