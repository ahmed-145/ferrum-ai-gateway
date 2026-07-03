# Stage 1: Dependency planner (uses cargo-chef to fingerprint dependencies)
FROM rust:1.78-alpine AS planner
RUN apk add --no-cache musl-dev
RUN cargo install cargo-chef --locked
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Dependency cache layer (rarely invalidated)
FROM rust:1.78-alpine AS builder
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static ca-certificates
RUN rustup target add x86_64-unknown-linux-musl
RUN cargo install cargo-chef --locked
WORKDIR /app

# Copy dependency recipe and pre-build dependencies
COPY --from=planner /app/recipe.json recipe.json
RUN OPENSSL_STATIC=1 OPENSSL_LIB_DIR=/usr/lib OPENSSL_INCLUDE_DIR=/usr/include \
    cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Stage 3: Application compile (only re-runs when src/ changes, not deps)
COPY . .
RUN OPENSSL_STATIC=1 OPENSSL_LIB_DIR=/usr/lib OPENSSL_INCLUDE_DIR=/usr/include \
    cargo build --release --target x86_64-unknown-linux-musl

# Stage 4: Minimal runtime — FROM scratch for zero CVE surface
FROM scratch
# Copy CA certificates for TLS to upstream providers
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
# Copy the statically linked binary and default config
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/ferrum /ferrum
COPY --from=builder /app/ferrum.toml /ferrum.toml
EXPOSE 8080
ENV FERRUM_CONFIG_PATH=/ferrum.toml
ENTRYPOINT ["/ferrum"]
