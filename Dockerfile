# ---- Builder ----
FROM rust:1.88-bookworm AS builder

WORKDIR /app

# Cache dependencies by copying manifests first
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Copy full source and build
COPY . .
RUN cargo build --release

# ---- Runtime ----
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false appuser

COPY --from=builder /app/target/release/slack-mongo-bot /usr/local/bin/slack-mongo-bot
COPY --from=builder /app/schemas /app/schemas

WORKDIR /app
USER appuser

EXPOSE 3000

ENTRYPOINT ["slack-mongo-bot"]
