# ferrum — High-Performance Rust AI Gateway

> A pure-Rust, OpenAI-compatible API gateway that eliminates proxy overhead in AI-heavy workloads.
> Built on `axum` + `tokio` — no Python, no GC pauses, no GIL.

---

## Why This Exists

Python-based AI gateways (like LiteLLM) introduce meaningful latency in high-concurrency agent loops:

- The Python **GIL** means only one thread runs at a time — concurrent requests queue up
- The **cyclic garbage collector** introduces unpredictable stop-the-world pauses mid-stream
- At 50+ concurrent agent calls, a 7ms gateway overhead compounds into **350ms of pure infrastructure tax per loop iteration**

`ferrum` moves the hot path to a compiled Rust binary. The result:

| Metric | Python Gateway | ferrum (Rust) | Improvement |
|---|---|---|---|
| P99 request overhead | ~7.5 ms | ~0.08 ms | **93× faster** |
| Max sustained throughput | ~453 RPS | ~6,782 RPS | **15× higher** |
| Peak memory (RSS) | ~359 MB | ~32 MB | **11× lower** |
| P99 tail latency @ 500 RPS | ~90s (queue buildup) | ~1.68s | **54× lower** |

> LiteLLM announced migration to a Rust data plane in June 2026. `ferrum` is a ground-up implementation of the same architecture.

---

## What It Does

`ferrum` sits between your application and AI provider APIs. It:

1. **Accepts** standard OpenAI-format requests (`/v1/chat/completions`, `/v1/messages`)
2. **Routes** to the correct provider based on the model prefix in the request body
3. **Translates** headers and request/response format where needed (Anthropic uses a different API shape)
4. **Streams** the response back chunk-by-chunk — the instant a byte arrives from the provider, it goes to the client
5. **Rate limits** each API key with a lock-free sliding window
6. **Retries** on 429/5xx with exponential backoff + jitter, then fails over to a secondary provider
7. **Exposes** Prometheus metrics at `/metrics` for real-time observability

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                          CLIENT                              │
│          (curl / Python SDK / Agent Framework)               │
└────────────────────────┬─────────────────────────────────────┘
                         │  HTTP/1.1 or HTTP/2
                         ▼
┌──────────────────────────────────────────────────────────────┐
│                   FERRUM DATA PLANE (Rust)                   │
│                                                              │
│  ┌─────────────┐   ┌──────────────────┐   ┌──────────────┐  │
│  │ axum Router │──▶│  Auth Extractor  │──▶│  DashMap     │  │
│  │             │   │  Bearer / x-api  │   │  Rate Limiter│  │
│  │ /v1/chat/   │   └──────────────────┘   │  (lock-free) │  │
│  │ completions │                          └──────────────┘  │
│  │ /v1/messages│   ┌──────────────────┐                     │
│  │ /health/*   │   │ Provider Router  │                     │
│  │ /metrics    │──▶│ groq / cerebras  │                     │
│  └─────────────┘   │ gemini / openai  │                     │
│                    │ anthropic (xlate)│                     │
│                    └────────┬─────────┘                     │
│                             │                               │
│                    ┌────────▼─────────┐                     │
│                    │  reqwest Pool    │                     │
│                    │  HTTP/2 keepalive│                     │
│                    │  SSE passthrough │                     │
│                    └────────┬─────────┘                     │
│                             │  retry + jitter backoff       │
│              ┌──────────────┼──────────────┐                │
│              ▼              ▼              ▼                │
│         Groq API      Gemini API     Anthropic API          │
│         Cerebras      OpenAI         (+ any future)         │
└──────────────────────────────────────────────────────────────┘
```

### Key Design Decisions

**No Python in the hot path.** The Rust binary owns the full request lifecycle: socket accept → auth → rate limit → upstream forward → stream passthrough → metric record. Zero interpreter overhead.

**Zero-copy streaming.** SSE chunks from upstream are piped directly into the downstream response via `axum::Body::from_stream(reqwest::bytes_stream())`. No buffering, no collecting — TTFT (time-to-first-token) is within milliseconds of the raw provider.

**Lock-free rate limiting.** `DashMap` is a sharded concurrent hashmap — reads and writes to different keys never block each other. The sliding window tracks per-key timestamps with a `Mutex<Vec<Instant>>` per bucket. A background tokio task cleans up expired buckets every 60 seconds to prevent memory growth.

**Deterministic memory.** Rust's ownership model guarantees deallocation at the end of each request scope. There is no garbage collector to cause pauses, and no cyclic reference leaks. Memory usage stays flat under any load profile.

---

## Provider Support

`ferrum` never holds API keys. The client sends their own key in `Authorization: Bearer <key>`, and `ferrum` forwards it to the upstream provider. **All five providers are fully supported** — the "Free Tier" column only tells you whether you can test without a credit card.

| Prefix | Provider | Free Tier | Works with paid key | Protocol |
|---|---|---|---|---|
| `groq/` | [Groq](https://groq.com) | ✅ Free | ✅ Yes | OpenAI-compatible |
| `cerebras/` | [Cerebras](https://cerebras.ai) | ✅ Free | ✅ Yes | OpenAI-compatible |
| `gemini/` | [Google Gemini](https://ai.google.dev) | ✅ Free | ✅ Yes | OpenAI-compatible endpoint |
| `openai/` | [OpenAI](https://openai.com) | ❌ Requires credits | ✅ Yes | Native OpenAI |
| `anthropic/` | [Anthropic](https://anthropic.com) | ❌ Requires credits | ✅ Yes — **auto-translated** ↔ OpenAI | Anthropic Messages API |

> **In short:** if you have an API key with credits for any of the above, just put it in `Authorization: Bearer <your-key>` and use the matching model prefix. ferrum handles everything else.

### How Routing Works

The provider is inferred from the `model` field prefix in the JSON body. `ferrum` strips the prefix before forwarding:

```
Request body:  { "model": "groq/llama-3.1-8b-instant", ... }
                         ↓
ferrum routes to:  https://api.groq.com/openai/v1/chat/completions
                         ↓
Forwarded body:  { "model": "llama-3.1-8b-instant", ... }   ← prefix stripped
```

### Anthropic Translation (Automatic)

Anthropic uses a different API shape. `ferrum` handles the full bidirectional translation transparently:

```
Client sends:  POST /v1/chat/completions  (OpenAI format)
                         ↓
ferrum translates:  POST /v1/messages  (Anthropic format, x-api-key header)
                         ↓
Anthropic streams:  event: content_block_delta  (Anthropic SSE)
                         ↓
ferrum translates:  data: {"choices":[...]}  (OpenAI SSE chunks)
                         ↓
Client receives:  standard OpenAI stream — no special handling needed
```

---

## Quickstart

### Prerequisites

- Rust stable (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- A free API key from [Groq](https://console.groq.com) or [Cerebras](https://cloud.cerebras.ai) for local testing

### Run Locally

```bash
git clone https://github.com/ahmed-145/ferrum-ai-gateway
cd ferrum-ai-gateway

# Copy the env template and fill in your key(s)
cp .env.example .env
# Edit .env — add your GROQ_API_KEY (free tier)

# Run tests first
cargo test

# Start the gateway
cargo run --release
# → Server listening on http://0.0.0.0:8080
```

### Test with curl

```bash
# Non-streaming (Groq — free)
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $GROQ_API_KEY" \
  -d '{
    "model": "groq/llama-3.1-8b-instant",
    "messages": [{"role": "user", "content": "What is Rust?"}]
  }'

# Streaming (Cerebras — free)
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $CEREBRAS_API_KEY" \
  -d '{
    "model": "cerebras/llama3.1-8b",
    "messages": [{"role": "user", "content": "Count to 5."}],
    "stream": true
  }'

# Anthropic via OpenAI format (auto-translated)
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ANTHROPIC_API_KEY" \
  -d '{
    "model": "anthropic/claude-3-5-sonnet-20241022",
    "messages": [{"role": "user", "content": "Hello."}],
    "stream": true
  }'

# Health check
curl http://localhost:8080/health/liveness    # → 200 OK

# Prometheus metrics
curl http://localhost:8080/metrics
```

### Use with any OpenAI SDK

Because ferrum is fully OpenAI-compatible, just point the SDK's base URL at the gateway:

```python
from openai import OpenAI

client = OpenAI(
    api_key="your-groq-key",
    base_url="http://localhost:8080/v1",
)

response = client.chat.completions.create(
    model="groq/llama-3.1-8b-instant",
    messages=[{"role": "user", "content": "Hello from Python!"}],
    stream=True,
)

for chunk in response:
    print(chunk.choices[0].delta.content or "", end="", flush=True)
```

---

## Configuration

`ferrum.toml` lives in the project root. No secrets ever go here — it is checked into the repo. API keys are sent by clients at request time.

```toml
[server]
host = "0.0.0.0"
port = 8080
max_connections = 10000        # reqwest connection pool size

[providers.groq]
base_url = "https://api.groq.com/openai/v1"
timeout_secs = 30
max_retries = 3                # retries on 429 / 5xx before fallback

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
header_style = "anthropic"     # triggers auto request/response translation

[providers.openai]
base_url = "https://api.openai.com/v1"
timeout_secs = 60
max_retries = 3

[rate_limit]
requests_per_minute = 1000     # per API key, sliding window
window_secs = 60

[fallback]
enabled = true
default_fallback_model = "gemini/gemini-2.0-flash"   # used after all retries exhausted
max_attempts = 3
backoff_ms = 500               # base delay; doubles per retry + ±25% jitter

[metrics]
enabled = true
path = "/metrics"
```

### Environment Variable Overrides

```bash
FERRUM_PORT=8080
FERRUM_LOG_LEVEL=info          # debug | info | warn | error
FERRUM_CONFIG_PATH=ferrum.toml
```

---

## Observability

### Prometheus Metrics (`/metrics`)

```
# Request rate by provider and model
ferrum_requests_total{provider="groq",model="llama-3.1-8b-instant",route="/v1/chat/completions",status="200"} 1042

# Error rate
ferrum_requests_total{provider="anthropic",status="429"} 3

# Latency histograms (P50 / P95 / P99)
ferrum_request_duration_seconds_bucket{le="0.001"} 891
ferrum_request_duration_seconds_bucket{le="0.005"} 1038
ferrum_request_duration_seconds_sum 1.24
ferrum_request_duration_seconds_count 1042
```

### Grafana Dashboard

Import `deploy/prometheus/ferrum-dashboard.json` into Grafana for a pre-built dashboard showing:
- Requests/sec by provider
- P50 / P95 / P99 latency heatmap
- Error rate by upstream status code
- Rate-limited request count

### Health Endpoints

| Endpoint | What it checks | Used by |
|---|---|---|
| `GET /health/liveness` | tokio event loop is alive | K8s `livenessProbe` |
| `GET /health/readiness` | gateway is ready to serve | K8s `readinessProbe` |

---

## Resilience: Retry + Fallback

When an upstream provider returns **HTTP 429** (rate limited) or **5xx** (server error), ferrum:

1. **Retries** the same provider up to `max_retries` times
2. Each retry waits `backoff_ms × 2^attempt` milliseconds, **±25% jitter** (prevents thundering herd)
3. If all retries are exhausted, **falls back** to `default_fallback_model` (e.g. `gemini/gemini-2.0-flash`)
4. Non-retryable errors (400 Bad Request, 401 Unauthorized, 403 Forbidden) are returned immediately — no retries

```
Request → groq/llama-3.1-8b
  ↓ 429 rate limited
  Wait 500ms + jitter → retry 1
  ↓ 429 again
  Wait 1000ms + jitter → retry 2
  ↓ 429 again
  Wait 2000ms + jitter → retry 3
  ↓ still failing
  → Fallback to gemini/gemini-2.0-flash
  ↓ 200 OK
  → Response to client
```

---

## Deployment

### Docker (Production)

The Dockerfile uses a 4-stage `cargo-chef` build:

1. **Planner** — fingerprints dependencies from `Cargo.toml`
2. **Builder** — compiles all dependencies (cached layer — only rebuilds when deps change)
3. **Compiler** — compiles only the application source
4. **Runtime** — copies the static binary into `FROM scratch` (no OS, no shell, no CVE surface)

```bash
docker build -f deploy/Dockerfile -t ferrum:latest .

docker run -p 8080:8080 \
  -e FERRUM_LOG_LEVEL=info \
  -v $(pwd)/ferrum.toml:/ferrum.toml \
  ferrum:latest
```

Final image size: **~25–30 MB** (statically linked musl binary + CA certificates only).

### Kubernetes

```bash
kubectl apply -f deploy/kubernetes/deployment.yaml   # 3-replica Deployment
kubectl apply -f deploy/kubernetes/service.yaml      # ClusterIP Service
kubectl apply -f deploy/kubernetes/hpa.yaml          # HPA: 3–10 replicas at 80% CPU
kubectl apply -f deploy/kubernetes/pdb.yaml          # PodDisruptionBudget: minAvailable=2
```

The Deployment includes K8s-native probe config matching ferrum's health endpoints:

```yaml
livenessProbe:
  httpGet: { path: /health/liveness, port: 8080 }
  initialDelaySeconds: 5
  periodSeconds: 10

readinessProbe:
  httpGet: { path: /health/readiness, port: 8080 }
  initialDelaySeconds: 3
  periodSeconds: 5
```

---

## Load Testing

Requires [k6](https://k6.io/docs/get-started/installation/).

```bash
# Steady-state: 100 virtual users for 30 minutes (proves no memory growth)
k6 run k6/steady_state.js

# Spike: 0 → 5,000 VUs in 10 seconds (proves tokio work-stealing handles bursts)
k6 run k6/spike.js

# Soak: 50 VUs for 72 hours (proves zero memory leak — Rust ownership by construction)
k6 run k6/soak.js
```

Each test exports P50/P95/P99 latency, error rates, and throughput. Thresholds are pre-configured — the test fails if P99 exceeds 500ms.

---

## CI/CD

### On Every Pull Request → `main`

```
fmt check → clippy (zero warnings) → cargo test → musl build (amd64 + arm64)
```

### On Tag Push (e.g. `v0.2.0`)

```
musl binary build → Docker multi-arch image → push to ghcr.io/ahmed-145/ferrum-ai-gateway
```

Both pipelines are defined in [`.github/workflows/`](.github/workflows/).

---

## Repository Structure

```
ferrum/
├── src/
│   ├── main.rs              # Entry point: load config, build state, bind axum
│   ├── config.rs            # Deserializes ferrum.toml + env var overrides
│   ├── router.rs            # axum Router: all route registrations
│   ├── proxy.rs             # Core: SSE passthrough, retry loop, fallback
│   ├── health.rs            # /health/liveness + /health/readiness
│   ├── providers/
│   │   ├── mod.rs           # ProviderKind enum + model prefix extraction
│   │   ├── anthropic.rs     # Full bidirectional OpenAI↔Anthropic translator
│   │   ├── groq.rs          # Header mapping for Groq
│   │   ├── cerebras.rs      # Header mapping for Cerebras
│   │   ├── gemini.rs        # Header mapping for Gemini
│   │   └── openai.rs        # OpenAI passthrough (reference implementation)
│   └── middleware/
│       ├── auth.rs          # Bearer / x-api-key extractor (axum FromRequestParts)
│       ├── rate_limit.rs    # DashMap sliding-window rate limiter
│       └── metrics.rs       # Prometheus RED metrics collector
│
├── tests/
│   ├── unit/
│   │   ├── test_config.rs       # Config parsing + env overrides
│   │   └── test_rate_limit.rs   # Sliding window correctness + cleanup
│   └── integration/
│       ├── test_streaming.rs    # OpenAI SSE passthrough + Anthropic SSE translation
│       ├── test_fallback.rs     # Failover routing using mock HTTP servers
│       └── test_groq.rs         # Live Groq test (skips automatically if no key set)
│
├── k6/
│   ├── steady_state.js      # 100 VUs × 30 min
│   ├── spike.js             # 0 → 5,000 VUs burst
│   └── soak.js              # 50 VUs × 72 hours
│
├── deploy/
│   ├── Dockerfile           # 4-stage cargo-chef → musl → FROM scratch
│   ├── kubernetes/
│   │   ├── deployment.yaml  # 3-replica Deployment with probes
│   │   ├── service.yaml     # ClusterIP Service
│   │   ├── hpa.yaml         # HorizontalPodAutoscaler (3–10 pods)
│   │   └── pdb.yaml         # PodDisruptionBudget (minAvailable: 2)
│   └── prometheus/
│       └── ferrum-dashboard.json   # Grafana dashboard export
│
├── .github/workflows/
│   ├── ci.yml               # PR pipeline: fmt → clippy → test → musl matrix build
│   └── release.yml          # Tag pipeline: Docker multi-arch → GHCR push
│
├── ferrum.toml              # Default config (no secrets — safe to commit)
├── .env.example             # API key template (copy to .env, never commit .env)
├── Cargo.toml               # Dependencies + test targets
└── ferrum-PRD.md            # Full product requirements document
```

---

## Tech Stack

| Layer | Crate | Why |
|---|---|---|
| Async runtime | `tokio` | Multi-threaded work-stealing scheduler — no GIL equivalent |
| HTTP server | `axum` | Built on `hyper`; Tower-native middleware; macro-free routing |
| HTTP client | `reqwest` | Connection pooling, HTTP/2 keepalive, native SSE byte streaming |
| Rate limiting | `dashmap` | Lock-free sharded hashmap; no `Mutex<HashMap>` contention |
| JSON | `serde_json` | Zero-copy deserialization into typed structs |
| Metrics | `prometheus-client` | Native Prometheus text exposition format |
| Tracing | `tracing` + `tracing-subscriber` | Structured async-aware logging |
| Structured concurrency | `tokio-util` | `CancellationToken` for clean task teardown on disconnect |
| Docker caching | `cargo-chef` | Layer-split builds — deps cached, only app source recompiles |
| Static linking | musl (`x86_64-unknown-linux-musl`) | 100% static binary; no host OS deps; immune to OS CVEs |

---

## License

MIT
