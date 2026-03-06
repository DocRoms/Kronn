# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1-slim AS builder
WORKDIR /build

# Cache dependencies
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && cargo build --release && rm -rf src

# Build app
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git curl nodejs npm python3 python3-pip && rm -rf /var/lib/apt/lists/* \
    && curl -LsSf https://astral.sh/uv/install.sh | sh

COPY --from=builder /build/target/release/kronn /usr/local/bin/kronn
COPY entrypoint.sh /usr/local/bin/entrypoint.sh

ENV KRONN_DATA_DIR=/data
ENV PATH="/root/.local/bin:${PATH}"
EXPOSE 3140

ENTRYPOINT ["entrypoint.sh"]
CMD ["kronn"]
