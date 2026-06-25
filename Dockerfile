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
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/server/target/release/bytehangar /usr/local/bin/bytehangar
# Public (edge) and internal planes.
EXPOSE 5100 5101
# Bind the internal plane on all interfaces inside the container; restrict exposure
# at the network/orchestration layer.
ENV INTERNAL_BIND_ADDRESS=0.0.0.0
ENTRYPOINT ["/usr/local/bin/bytehangar"]
