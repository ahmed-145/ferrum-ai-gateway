# PRD: `ferrum` — High-Performance Rust AI Gateway

> **Status:** Draft v1.0  
> **Author:** \[Your Name\]  
> **Last Updated:** June 2026  
> **Target Roles:** Backend/Systems Engineer · DevOps/Platform Engineer

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Background & Market Context](#2-background--market-context)
3. [Goals & Non-Goals](#3-goals--non-goals)
4. [Architecture Overview](#4-architecture-overview)
5. [Functional Requirements](#5-functional-requirements)
6. [Performance Requirements](#6-performance-requirements)
7. [Technical Stack](#7-technical-stack)
8. [Phased Delivery Plan](#8-phased-delivery-plan)
9. [DevOps & Platform Layer](#9-devops--platform-layer)
10. [Key Technical Risks & Mitigations](#10-key-technical-risks--mitigations)
11. [Competitive Landscape](#11-competitive-landscape)
12. [Success Metrics (CV Evidence)](#12-success-metrics-cv-evidence)
13. [Supported Providers](#13-supported-providers)
14. [Repository Structure](#14-repository-structure)
15. [Configuration Spec](#15-configuration-spec)
16. [Claude Code Prompt (Phase 1)](#16-claude-code-prompt-phase-1)

---

## 1. Problem Statement

Current Python-based AI API gateways introduce unacceptable compounding latency, memory bloat, and systemic instability when subjected to high-concurrency agentic workflows. The architectural reliance on the Python Global Interpreter Lock (GIL) and cyclic garbage collection results in P99 latency variances that fundamentally disrupt real-time autonomous agent loops and precipitate catastrophic Out-Of-Memory (OOM) cascading failures under sudden spike loads.

In dense agent loops — where a single user query may trigger 20–50 sequential or parallel LLM calls — a Python gateway injecting 100ms of P99 overhead per call compounds into 5 seconds of pure infrastructure latency, entirely distinct from the LLM provider's own time-to-first-token. This project eliminates that overhead by moving the hot path to a compiled Rust binary.

---

## 2. Background & Market Context

### Why Python Gateways Break at Scale

| Root Cause | Effect at Scale |
|---|---|
| Global Interpreter Lock (GIL) | Only one thread executes Python bytecode at a time; CPU-bound tasks (JSON parsing, token counting) block the event loop |
| Cyclic Garbage Collector | Unpredictable stop-the-world pauses; cyclic references in async tasks cause memory growth under SSE streaming |
| Reference Counting Under Load | Allocator pressure from processing thousands of SSE chunks/second leads to heap fragmentation |
| OOM Kills on K8s Pods | Gateway pods are forcibly terminated mid-stream, dropping in-flight LLM requests after seconds of upstream cost |

### Benchmark Reality Check

| Metric | Python Gateway (LiteLLM Stage 0) | Compiled Gateway (Rust) | Delta |
|---|---|---|---|
| Per-request proxy overhead | ~7.5 ms | ~0.05 ms | **150x faster** |
| Max sustained throughput | ~453 RPS | ~6,782 RPS | **15x higher** |
| Peak memory footprint | ~359 MB | ~32 MB | **11x lower** |
| P99 latency @ 500 RPS | ~90.72s (queue buildup) | ~1.68s | **54x lower tail latency** |

> Source: LiteLLM internal migration analysis (June 2026) and Bifrost/Maxim AI benchmarks.

### The Industry Signal

LiteLLM — the dominant open-source AI gateway with 100+ provider integrations — announced in June 2026 a full migration to a Rust data plane via PyO3/axum, targeting completion by December 2026. This project is a focused, CV-grade implementation of the same architecture, built from first principles.

---

## 3. Goals & Non-Goals

### Goals

- Build a pure Rust, OpenAI-compatible proxy gateway with sub-millisecond per-request overhead
- Implement deterministic, zero-allocation SSE streaming passthrough
- Provide lock-free, hierarchical rate limiting via `DashMap`
- Deliver a production-quality DevOps pipeline: multi-stage Docker (<30MB), Kubernetes manifests, Prometheus metrics, k6 load tests
- Generate reproducible benchmark evidence suitable for engineering interviews

### Non-Goals

- Full LLMOps GUI / dashboard
- Integrated vector database or semantic caching layer
- Support for the long tail of obscure AI providers (focus: OpenAI, Anthropic, AWS Bedrock)
- Python plugin sidecar architecture (scope: pure Rust data plane only)
- Multi-tenant billing UI

---

## 4. Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        CLIENT                               │
│   (curl / Python SDK / Agent Framework / Claude Code)       │
└───────────────────────┬─────────────────────────────────────┘
                        │ HTTP/1.1 + HTTP/2
                        ▼
┌─────────────────────────────────────────────────────────────┐
│                   RUST DATA PLANE                           │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ axum Router  │  │ Tower Layers │  │  DashMap         │  │
│  │              │→ │ - Auth       │→ │  Rate Limiter    │  │
│  │ /v1/chat/    │  │ - Timeout    │  │  (lock-free)     │  │
│  │ completions  │  │ - Retry      │  └──────────────────┘  │
│  │ /v1/messages │  │ - Tracing    │                        │
│  └──────────────┘  └──────┬───────┘  ┌──────────────────┐  │
│                           │          │ Prometheus /metrics│  │
│                           ▼          └──────────────────┘  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              reqwest Connection Pool                  │  │
│  │    (keep-alive TLS, SSE stream passthrough)          │  │
│  └──────────────────────────────────────────────────────┘  │
└───────────────────────┬─────────────────────────────────────┘
                        │ TLS / HTTP/2
           ┌────────────┼────────────┐
           ▼            ▼            ▼
      OpenAI API   Anthropic API  AWS Bedrock
```

### Key Design Principles

**Data plane only.** The Rust binary owns: socket connections, TLS termination, protocol normalization, concurrent streaming, rate limiting. No Python in the hot path.

**Bounded memory by construction.** Rust's ownership model guarantees deterministic deallocation. Fixed-size chunk streaming prevents heap growth regardless of payload size.

**Zero-copy where possible.** SSE chunks are piped directly from the upstream socket to the downstream client via `axum::Body::from_stream`, bypassing heap allocation for passthrough bytes.

---

## 5. Functional Requirements

### FR-1: OpenAI-Compatible Ingress
The gateway accepts standard `/v1/chat/completions` and `/v1/messages` JSON payloads and routes them to the configured upstream provider. Response schemas are normalized back to OpenAI format.

### FR-2: Deterministic SSE Streaming Passthrough
Streaming completions must be forwarded with minimal buffering. The instant a byte chunk arrives from upstream, it is flushed to the downstream client. TTFT must be within 5ms of querying the provider directly.

### FR-3: Lock-Free Rate Limiting
A sliding-window rate limiting algorithm implemented via `DashMap`, segmented hierarchically:
- Per virtual API key
- Per organizational tenant
- Per upstream provider quota

### FR-4: Intelligent Fallback Routing
On upstream HTTP 429 or 5xx responses, automatic failover to a configured secondary model (e.g., `openai/gpt-4o` → `anthropic/claude-3-5-sonnet`) with exponential backoff and jitter.

### FR-5: Health & Readiness Endpoints
- `GET /health/liveness` — non-blocking tokio runtime check
- `GET /health/readiness` — verifies connectivity to backing services (Redis/Postgres if configured)

### FR-6: Prometheus RED Metrics
Exposed on `GET /metrics` (Prometheus text format):
- **Rate:** requests per second by provider, model, route
- **Errors:** 4xx/5xx counters by upstream
- **Duration:** latency histograms at P50/P95/P99

---

## 6. Performance Requirements

| Requirement | Target | Test Method |
|---|---|---|
| Per-request proxy overhead (P99) | < 500µs | k6 steady-state @ 1,000 RPS |
| Peak memory (RSS) | < 50 MB | k6 soak test, 72 hours |
| Max sustained throughput | > 5,000 RPS | k6 spike test |
| Docker image size | < 30 MB | `docker image inspect` |
| Time-To-First-Token overhead | < 5ms added | Streaming latency comparison |
| Memory growth over 72h soak | Zero (flat) | Prometheus RSS gauge |

---

## 7. Technical Stack

### Core Runtime

| Component | Crate / Tool | Rationale |
|---|---|---|
| Async runtime | `tokio` | Multi-threaded work-stealing scheduler; no GIL equivalent |
| HTTP framework | `axum` | Built on `hyper`; macro-free routing; Tower-native |
| Outbound HTTP | `reqwest` | Connection pooling, keep-alive TLS, native SSE/streaming |
| Concurrent rate limiting | `dashmap` | Lock-free sharded hashmap; 42x better memory vs Mutex<HashMap> |
| Middleware | `tower` + `tower-http` | Composable Service/Layer traits; zero-cost at compile time |
| JSON | `serde_json` + `simd-json` | SIMD-accelerated deserialization for hot paths |
| Metrics | `prometheus-client` or `metrics` | Native Prometheus exposition format |
| Tracing | `tracing` + `tracing-opentelemetry` | Distributed trace propagation (W3C traceparent) |
| Cancellation | `tokio-util::sync::CancellationToken` | Structured concurrency; clean task teardown on disconnect |

### DevOps Stack

| Component | Tool | Rationale |
|---|---|---|
| Dependency caching | `cargo-chef` | Layer-split Docker builds; CI rebuild in seconds not minutes |
| Static linking | `musl` (`x86_64-unknown-linux-musl`) | 100% static binary; no host OS deps; immune to OS CVEs |
| Container base | `scratch` or `alpine` | Sub-30MB final image |
| Orchestration | Kubernetes + Helm | Liveness/readiness probes; graceful SIGTERM handling |
| CI/CD | GitHub Actions | Matrix builds: linux/amd64 + linux/arm64 |
| Load testing | Grafana k6 | Steady-state, spike, soak test suites |
| Profiling | `cargo flamegraph` + `perf` | Prove zero lock contention in hot paths |

---

## 8. Phased Delivery Plan

### Phase 1 — Fast Path MVP (Weeks 1–3)

**Goal:** A working proxy that handles unary and streaming requests byte-for-byte correctly.

- `axum` router with `/v1/chat/completions` and `/v1/messages` routes
- `reqwest` connection pool to OpenAI and Anthropic upstreams
- SSE streaming passthrough via `Body::from_stream`
- Basic API key auth (header extraction)
- `/health/liveness` endpoint

**Milestone:** `curl` through the gateway returns a correct streaming response from OpenAI. Latency overhead measurably below 1ms on localhost.

---

### Phase 2 — Resilience & Observability (Weeks 4–6)

**Goal:** Production-hardened middleware stack with full metrics.

- Tower middleware: configurable timeouts, exponential-backoff retries with jitter
- Circuit breaker pattern for 429/5xx failover between providers
- `DashMap` sliding-window rate limiter (per-key, per-tenant, per-provider)
- Prometheus RED metrics via `/metrics`
- OpenTelemetry trace propagation (W3C `traceparent`)
- Graceful shutdown via `tokio::signal::ctrl_c` / `SIGTERM`

**Milestone:** k6 steady-state test @ 1,000 RPS shows P99 < 500µs. Memory flat over 30-minute soak.

---

### Phase 3 — Production DevOps Pipeline (Weeks 7–9)

**Goal:** Deployable, benchmarked, CI-built artifact ready to show in interviews.

- `cargo-chef` multi-stage `Dockerfile` → sub-30MB image
- Kubernetes manifests: `Deployment`, `Service`, `HorizontalPodAutoscaler`, `PodDisruptionBudget`
- Liveness + readiness probe configuration
- GitHub Actions pipeline: lint → test → build musl binary → build Docker → push to GHCR
- k6 load test suite (steady-state / spike / soak) with exported Grafana dashboards
- `cargo flamegraph` execution profiles committed to repo
- `README.md` with architecture diagram, benchmark results table, and quick-start

**Milestone:** Public GitHub repo with green CI, benchmark report, and <30MB Docker image on GHCR.

---

## 9. DevOps & Platform Layer

### Multi-Stage Docker Build Strategy

```dockerfile
# Stage 1: Dependency planner
FROM rust:1.78-alpine AS planner
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Dependency cache (rarely invalidated)
FROM rust:1.78-alpine AS builder
RUN apk add --no-cache musl-dev openssl-dev
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Stage 3: Application compile
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

# Stage 4: Minimal runtime image
FROM scratch
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/ferrum /ferrum
EXPOSE 8080
ENTRYPOINT ["/ferrum"]
```

**Result:** ~25–30MB final image, no OS-level CVE surface, boots in <50ms.

### Kubernetes Probe Design

```yaml
livenessProbe:
  httpGet:
    path: /health/liveness
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 10
  # Verifies: tokio event loop is not deadlocked

readinessProbe:
  httpGet:
    path: /health/readiness
    port: 8080
  initialDelaySeconds: 3
  periodSeconds: 5
  # Verifies: upstream provider DNS resolves, Redis reachable (if configured)
```

### k6 Load Test Scenarios

```javascript
// Steady-state: prove no memory growth
export const steadyState = { executor: 'constant-vus', vus: 100, duration: '30m' };

// Spike: prove tokio work-stealing handles burst
export const spike = { executor: 'ramping-vus', stages: [
  { duration: '10s', target: 5000 },
  { duration: '1m', target: 5000 },
  { duration: '10s', target: 0 },
]};

// Soak: prove zero memory leak over time
export const soak = { executor: 'constant-vus', vus: 50, duration: '72h' };
```

---

## 10. Key Technical Risks & Mitigations

### Risk 1: PyO3 GIL Deadlocks (if FFI bridge is added)

**Problem:** Calling async Rust from a Python thread while holding the GIL causes permanent deadlock if tokio needs that thread for another task.

**Mitigation:** All Rust functions callable from Python must release the GIL before awaiting futures:
```rust
py.allow_threads(|| {
    tokio_runtime.block_on(async { /* network I/O here */ })
})
```
In PyO3 0.23+, use `Python::detach()`. Track free-threaded CPython (3.14+) for eventual GIL removal.

---

### Risk 2: SSE Buffering Destroying TTFT

**Problem:** If the proxy accidentally buffers the full upstream response before forwarding, TTFT is destroyed and memory spikes.

**Mitigation:** Use `reqwest`'s `bytes_stream()` piped directly into `axum::Body::from_stream`. Never collect the stream. Force `Transfer-Encoding: chunked` on the downstream response. For token counting, use zero-copy inspection on transient chunk slices without allocating new strings.

---

### Risk 3: CI Cross-Compilation Fragility

**Problem:** musl static builds in GitHub Actions break on C-linker dependencies (OpenSSL in particular).

**Mitigation:** Build inside `ghcr.io/cross-rs/x86_64-unknown-linux-musl` Docker image. Use `openssl` crate with `vendored` feature flag to statically link OpenSSL. For ARM64 targets, use `docker/setup-qemu-action` + `maturin-action` with zig as cross-linker.

---

### Risk 4: Orphaned Tasks on Client Disconnect

**Problem:** When a client disconnects mid-stream, background tasks (cost ledger writes, rate-limit bucket updates) may be silently dropped or become zombie tasks.

**Mitigation:** Use `tokio_util::sync::CancellationToken` scoped to each request's lifetime. Wrap critical state mutations (token ledger writes) in a `Drop` guard pattern to guarantee completion even on cancellation. Never `tokio::spawn` unbounded tasks without a parent scope.

---

### Risk 5: DashMap Contention at Extreme Scale

**Problem:** At >10,000 concurrent requests with high key cardinality, even sharded maps can show contention on hot shards.

**Mitigation:** Tune `DashMap` shard count to `num_cpus * 4`. For known hot keys (shared tenant limits), pre-shard by hashing the key prefix. Validate with `cargo flamegraph` — contention shows as wide stack frames in the lock acquisition path.

---

## 11. Competitive Landscape

| Platform | Language | Overhead | Throughput | Key Differentiator |
|---|---|---|---|---|
| **LiteLLM** (current) | Python → Rust (Dec 2026) | ~7.5ms → <1ms | 453 → 6,782 RPS | 100+ provider integrations |
| **Bifrost** (Maxim AI) | Go | 11µs | 5,000+ RPS | MCP gateway, semantic caching |
| **fast-litellm** (Neul Labs) | Rust via PyO3 | Partial improvement | 3.2x pool speedup | Drop-in Python patch |
| **litellm-rust** (crates.io) | Pure Rust | Low | High | Full Rust SDK reimplementation |
| **`ferrum` (this project)** | Pure Rust | **<500µs target** | **>5,000 RPS target** | Pure data plane, DevOps-complete, CV-grade benchmarks |

### Open Differentiation Angles

These are features none of the existing projects nail simultaneously — valuable future scope:

- **Zero-copy SIMD JSON inspection** — parse SSE chunks on the fly without heap allocation using `simd-json`
- **WASM plugin sandbox** — embed `wasmtime` to allow custom guardrails/routing logic in any language, avoiding Python sidecar IPC overhead entirely
- **Agentic connection affinity** — recognize session IDs from agent frameworks (LangGraph, AutoGen) and pin TCP connections to upstream shards for deep tool-calling loops

---

## 12. Success Metrics (CV Evidence)

These are the concrete artifacts that make this project interview-ready:

| Artifact | What It Proves |
|---|---|
| k6 benchmark report (HTML export) | >5,000 RPS at <1ms P99 overhead — quantified, reproducible |
| `cargo flamegraph` profile | Zero lock contention in rate limiter and router hot paths |
| Prometheus dashboard screenshot | Flat RSS over 72-hour soak — zero memory leak |
| GitHub Actions CI badge | Multi-arch build pipeline: lint → test → musl binary → Docker push |
| `docker pull ghcr.io/[you]/ferrum` | Sub-30MB image, verifiable with `docker image inspect` |
| Architecture diagram in README | Shows data plane / control plane separation, Tower middleware chain |

### CV Bullet Points (Ready to Copy)

```
• Built a pure Rust AI gateway (axum + tokio) achieving <500µs P99 proxy overhead
  at 5,000+ RPS, vs ~7.5ms baseline in Python — a 15x throughput improvement

• Implemented lock-free sliding-window rate limiting via DashMap, eliminating
  Mutex contention across 10,000+ concurrent API key evaluations

• Engineered cargo-chef multi-stage Docker pipeline producing a 28MB statically
  linked musl binary; deployed on Kubernetes with liveness/readiness probe design

• Validated zero memory growth over 72-hour soak test via Prometheus RSS gauge,
  demonstrating Rust's ownership model eliminates OOM risk by construction

• Built GitHub Actions CI matrix outputting linux/amd64 + linux/arm64 artifacts
  with automated k6 load test execution and Grafana dashboard export
```

---

*This PRD is designed to be committed to the root of the `ferrum` GitHub repository as `PRD.md`. It doubles as both a development specification and a portfolio artifact demonstrating product thinking alongside engineering depth.*

---

## 13. Supported Providers

Ferrum is a passthrough proxy — **it never holds API keys**. The user sends their own key in the `Authorization: Bearer <key>` header, and ferrum forwards it to the upstream provider. The operator running ferrum needs zero provider accounts.

### Priority Provider Support (Phase 1)

| Provider | Base URL | Free Tier | OpenAI-Compatible | Notes |
|---|---|---|---|---|
| **Groq** | `https://api.groq.com/openai/v1` | ✅ Yes | ✅ Yes | Fastest tokens/sec available; best for streaming tests |
| **Cerebras** | `https://api.cerebras.ai/v1` | ✅ Yes | ✅ Yes | Custom silicon; lowest latency for small models |
| **Google Gemini** | `https://generativelanguage.googleapis.com/v1beta/openai` | ✅ Yes | ✅ Yes | Via OpenAI-compat endpoint |
| **Anthropic** | `https://api.anthropic.com/v1` | ❌ Paid | ⚠️ Partial | Requires header translation (`x-api-key`) |
| **OpenAI** | `https://api.openai.com/v1` | ❌ Paid | ✅ Native | Reference implementation |

> **Developer note:** Use Groq and Cerebras for all local development and CI testing. Both are free, fast, and fully OpenAI-compatible — no translation layer needed. Anthropic requires a small header normalization step (Phase 2).

### How Provider Routing Works

The user specifies the provider in their request via the model name prefix:

```bash
# Route to Groq
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $GROQ_API_KEY" \
  -d '{"model": "groq/llama-3.1-8b-instant", "messages": [...]}'

# Route to Cerebras
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $CEREBRAS_API_KEY" \
  -d '{"model": "cerebras/llama3.1-8b", "messages": [...]}'

# Route to Gemini
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $GEMINI_API_KEY" \
  -d '{"model": "gemini/gemini-2.0-flash", "messages": [...]}'
```

Ferrum strips the prefix, looks up the provider's base URL from config, and forwards the request with the user's key intact.

---

## 14. Repository Structure

Claude Code must generate exactly this folder layout. Do not deviate.

```
ferrum/
├── Cargo.toml                  # Workspace root
├── Cargo.lock
├── PRD.md                      # This document
├── README.md                   # Architecture diagram + quick-start + benchmarks
├── ferrum.toml                 # Default config (checked into repo, no secrets)
├── .env.example                # Documents env vars; never commit .env itself
│
├── src/
│   ├── main.rs                 # Entry point: loads config, starts tokio runtime, binds axum
│   ├── config.rs               # Deserializes ferrum.toml + env var overrides
│   ├── router.rs               # axum Router definition; all route registrations
│   ├── proxy.rs                # Core forwarding logic: reqwest pool, SSE passthrough
│   ├── providers/
│   │   ├── mod.rs              # Provider trait definition
│   │   ├── groq.rs             # Groq-specific URL + header mapping
│   │   ├── cerebras.rs         # Cerebras-specific URL + header mapping
│   │   ├── gemini.rs           # Gemini OpenAI-compat endpoint mapping
│   │   ├── anthropic.rs        # Anthropic header translation (x-api-key)
│   │   └── openai.rs           # OpenAI passthrough (reference)
│   ├── middleware/
│   │   ├── mod.rs
│   │   ├── auth.rs             # API key extraction from Authorization header
│   │   ├── rate_limit.rs       # DashMap sliding-window rate limiter
│   │   └── metrics.rs          # Prometheus RED instrumentation
│   └── health.rs               # /health/liveness + /health/readiness handlers
│
├── tests/
│   ├── integration/
│   │   ├── test_groq.rs        # Live integration test against Groq free tier
│   │   ├── test_streaming.rs   # SSE streaming correctness test
│   │   └── test_fallback.rs    # Failover routing test
│   └── unit/
│       ├── test_rate_limit.rs  # DashMap rate limiter unit tests
│       └── test_config.rs      # Config parsing unit tests
│
├── k6/
│   ├── steady_state.js         # 1,000 RPS for 30 minutes
│   ├── spike.js                # 0 → 5,000 RPS burst
│   └── soak.js                 # 50 VU for 72 hours
│
├── deploy/
│   ├── Dockerfile              # Multi-stage cargo-chef musl build
│   ├── kubernetes/
│   │   ├── deployment.yaml
│   │   ├── service.yaml
│   │   └── hpa.yaml            # HorizontalPodAutoscaler
│   └── prometheus/
│       └── ferrum-dashboard.json  # Grafana dashboard export
│
└── .github/
    └── workflows/
        ├── ci.yml              # lint + test + build on every PR
        └── release.yml         # musl binary + Docker push on tag
```

---

## 15. Configuration Spec

Ferrum is configured via `ferrum.toml` in the project root. No secrets ever go in this file — only structure. Secrets (API keys for testing) go in `.env` which is gitignored.

### `ferrum.toml` (checked into repo)

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
# Anthropic uses x-api-key header instead of Authorization: Bearer
header_style = "anthropic"

[providers.openai]
base_url = "https://api.openai.com/v1"
timeout_secs = 60
max_retries = 3

[rate_limit]
# Per API key, per minute
requests_per_minute = 1000
# Sliding window size in seconds
window_secs = 60

[metrics]
enabled = true
path = "/metrics"
```

### `.env.example` (checked into repo — template only)

```bash
# Copy this to .env and fill in your keys for local development
# NEVER commit .env

# Use free tier providers for development
GROQ_API_KEY=your_groq_key_here
CEREBRAS_API_KEY=your_cerebras_key_here
GEMINI_API_KEY=your_gemini_key_here

# Optional: only needed if testing paid providers
ANTHROPIC_API_KEY=
OPENAI_API_KEY=

# Override config file values via env if needed
FERRUM_PORT=8080
FERRUM_LOG_LEVEL=info
```

### Model Prefix → Provider Routing Table

| Request model string | Routed to | Key used |
|---|---|---|
| `groq/llama-3.1-8b-instant` | Groq | User's `Authorization: Bearer` header |
| `groq/mixtral-8x7b-32768` | Groq | User's `Authorization: Bearer` header |
| `cerebras/llama3.1-8b` | Cerebras | User's `Authorization: Bearer` header |
| `gemini/gemini-2.0-flash` | Gemini | User's `Authorization: Bearer` header |
| `anthropic/claude-3-5-sonnet` | Anthropic | User's `Authorization: Bearer` header |
| `openai/gpt-4o` | OpenAI | User's `Authorization: Bearer` header |

---

## 16. Claude Code Prompt (Phase 1)

Copy and paste this exactly into Claude Code to start Phase 1:

```
You are building "ferrum" — a high-performance Rust AI gateway.
Read PRD.md in full before writing any code.

## Your task: implement Phase 1 only

Build a working Rust HTTP proxy that:
1. Listens on 0.0.0.0:8080
2. Accepts POST /v1/chat/completions requests
3. Reads the model prefix from the request body (e.g. "groq/llama-3.1-8b-instant")
4. Looks up the provider's base_url from ferrum.toml
5. Forwards the request to the upstream provider with the user's Authorization header intact
6. Streams the SSE response back to the client byte-for-byte with zero buffering
7. Implements GET /health/liveness returning 200 OK

## Constraints
- Exact folder structure from PRD.md Section 14 — no deviations
- Cargo.toml dependencies: axum, tokio (full features), reqwest (stream feature), 
  serde, serde_json, toml, tower, tower-http, tracing, tracing-subscriber
- The model field in the request body must drive routing — strip the prefix before forwarding
- SSE streaming: pipe reqwest bytes_stream() directly into axum Body::from_stream
  — do NOT buffer or collect the stream
- No authentication in Phase 1 — just forward the user's Authorization header as-is
- No rate limiting in Phase 1
- No metrics in Phase 1
- Use stable Rust Edition 2021

## Test providers (free tier, no paid keys needed)
- Groq: https://api.groq.com/openai/v1 — model: "groq/llama-3.1-8b-instant"
- Cerebras: https://api.cerebras.ai/v1 — model: "cerebras/llama3.1-8b"
- Gemini: https://generativelanguage.googleapis.com/v1beta/openai — model: "gemini/gemini-2.0-flash"

## Done when
- cargo build --release succeeds with zero warnings
- curl test against Groq returns a streaming response through the proxy
- curl test against Cerebras returns a streaming response through the proxy
- cargo test passes
```
