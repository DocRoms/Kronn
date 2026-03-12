# syntax=docker/dockerfile:1
# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1-slim AS builder
WORKDIR /build

# CARGO_PROFILE: "release" for prod (LTO, slow), "fast" for dev (no LTO, ~4x faster)
ARG CARGO_PROFILE=release

# Cache dependencies: copy manifests + dummy build.
# BuildKit cache mounts keep cargo registry & target dir across rebuilds,
# so only changed crates recompile (not the full dep tree every time).
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && echo "" > src/lib.rs

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --profile ${CARGO_PROFILE} 2>/dev/null || true

# Build app (only our code recompiles, deps are cached)
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    touch src/main.rs src/lib.rs \
    && cargo build --profile ${CARGO_PROFILE} \
    && cp target/${CARGO_PROFILE}/kronn /usr/local/bin/kronn

# ── Lint stage (optional, used by `make lint-backend`) ───────────────────────
FROM builder AS linter
RUN rustup component add clippy
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo clippy --all-targets -- -D warnings

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:trixie-slim

# Create non-root user for runtime (UID/GID overridable via KRONN_HOST_UID/GID)
ARG APP_UID=1000
ARG APP_GID=1000
RUN groupadd -g ${APP_GID} kronn && useradd -u ${APP_UID} -g kronn -m kronn

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git curl nodejs npm python3 python3-pip \
    make gh \
    && rm -rf /var/lib/apt/lists/*

# Install uv as root, then make available to kronn user
RUN curl -LsSf https://astral.sh/uv/install.sh | sh \
    && cp /root/.local/bin/uv /usr/local/bin/uv \
    && cp /root/.local/bin/uvx /usr/local/bin/uvx

COPY --from=builder /usr/local/bin/kronn /usr/local/bin/kronn
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Data directory owned by kronn
RUN mkdir -p /data && chown kronn:kronn /data

# Pre-create directories that volumes will mount into
RUN mkdir -p /home/kronn/.local/bin /home/kronn/.local/share/uv \
    /home/kronn/.npm /home/kronn/.cache/uv \
    && chown -R kronn:kronn /home/kronn/.local /home/kronn/.npm /home/kronn/.cache

ENV KRONN_DATA_DIR=/data
ENV PATH="/home/kronn/.local/bin:/usr/local/bin:${PATH}"
EXPOSE 3140

USER kronn
ENTRYPOINT ["entrypoint.sh"]
CMD ["kronn"]
