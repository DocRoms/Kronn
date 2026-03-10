# syntax=docker/dockerfile:1
# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1-slim AS builder
WORKDIR /build

# Cache dependencies: copy manifests + dummy build.
# BuildKit cache mounts keep cargo registry & target dir across rebuilds,
# so only changed crates recompile (not the full dep tree every time).
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && echo "" > src/lib.rs

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release 2>/dev/null || true

# Build app (only our code recompiles, deps are cached)
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    touch src/main.rs src/lib.rs \
    && cargo build --release \
    && cp target/release/kronn /usr/local/bin/kronn

# ── Lint stage (optional, used by `make lint-backend`) ───────────────────────
FROM builder AS linter
RUN rustup component add clippy
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo clippy --all-targets -- -D warnings

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git curl nodejs npm python3 python3-pip \
    && rm -rf /var/lib/apt/lists/* \
    && curl -LsSf https://astral.sh/uv/install.sh | sh

COPY --from=builder /usr/local/bin/kronn /usr/local/bin/kronn
COPY entrypoint.sh /usr/local/bin/entrypoint.sh

ENV KRONN_DATA_DIR=/data
ENV PATH="/root/.local/bin:${PATH}"
EXPOSE 3140

ENTRYPOINT ["entrypoint.sh"]
CMD ["kronn"]
