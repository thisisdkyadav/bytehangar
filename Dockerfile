# Multi-stage build for the ByteHangar server.
FROM rust:1.94-bookworm AS build
WORKDIR /app
# Build dependencies first (cached) using the manifest + lockfile.
COPY server/Cargo.toml server/Cargo.lock ./server/
RUN mkdir -p server/src && echo 'fn main() {}' > server/src/main.rs \
    && cargo build --release --manifest-path server/Cargo.toml \
    && rm -rf server/src
# Now the real sources (migrations are embedded at compile time).
COPY server/src ./server/src
COPY server/migrations ./server/migrations
RUN touch server/src/main.rs \
    && cargo build --release --manifest-path server/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m -d /app appuser
COPY --from=build /app/server/target/release/bytehangar /usr/local/bin/bytehangar
USER appuser
WORKDIR /app
# Run unprivileged; keep state writable by appuser. Restrict plane exposure at the
# orchestration/network layer.
ENV INTERNAL_BIND_ADDRESS=0.0.0.0 \
    DATA_ROOT=/app/data
EXPOSE 5100 5101
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://localhost:${PORT:-5100}/health" || exit 1
ENTRYPOINT ["/usr/local/bin/bytehangar"]
