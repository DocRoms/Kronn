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
# On macOS the default GID is 20 which conflicts with 'dialout' in Debian — handle gracefully.
ARG APP_UID=1000
ARG APP_GID=1000
RUN if getent group ${APP_GID} >/dev/null 2>&1; then \
      existing=$(getent group ${APP_GID} | cut -d: -f1); \
      groupmod -n kronn "$existing"; \
    else \
      groupadd -g ${APP_GID} kronn; \
    fi \
    && useradd -u ${APP_UID} -g kronn -m kronn

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git curl nodejs npm python3 python3-pip \
    make gh openssh-client unzip \
    && rm -rf /var/lib/apt/lists/*

# Install glab (GitLab CLI) for MR creation
RUN ARCH=$(dpkg --print-architecture) && \
    curl -fsSL "https://gitlab.com/gitlab-org/cli/-/releases/v1.52.0/downloads/glab_1.52.0_linux_${ARCH}.deb" -o /tmp/glab.deb && \
    dpkg -i /tmp/glab.deb && rm /tmp/glab.deb

# Install Bun (needed by some MCP servers like fastly-mcp-server)
RUN BUN_ARCH=$(dpkg --print-architecture | sed 's/amd64/x64/' | sed 's/arm64/aarch64/') && \
    curl -fsSL "https://github.com/oven-sh/bun/releases/latest/download/bun-linux-${BUN_ARCH}.zip" -o /tmp/bun.zip && \
    unzip -o /tmp/bun.zip -d /tmp/bun && \
    mv /tmp/bun/bun-linux-${BUN_ARCH}/bun /usr/local/bin/bun && \
    chmod +x /usr/local/bin/bun && \
    rm -rf /tmp/bun /tmp/bun.zip

# Install uv (pinned version) as root, then make available to kronn user
COPY --from=ghcr.io/astral-sh/uv:0.5.14 /uv /usr/local/bin/uv
COPY --from=ghcr.io/astral-sh/uv:0.5.14 /uvx /usr/local/bin/uvx

COPY --from=builder /usr/local/bin/kronn /usr/local/bin/kronn
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
COPY scripts/ /app/scripts/

# Data directory owned by kronn
RUN mkdir -p /data && chown kronn:kronn /data

# Pre-create directories that volumes will mount into
RUN mkdir -p /home/kronn/.local/bin /home/kronn/.local/share/uv \
    /home/kronn/.npm /home/kronn/.cache/uv \
    && chown -R kronn:kronn /home/kronn/.local /home/kronn/.npm /home/kronn/.cache

# Allow kronn user to create symlinks in /home (for host-path compatibility).
# Agent configs mounted from the host may contain absolute host paths
# (e.g. Vibe's save_dir = "/home/<user>/.vibe/logs"). The entrypoint creates
# a symlink $KRONN_HOST_HOME → /home/kronn so these paths resolve correctly.
#
# Sticky bit (1777, like /tmp) lets non-root users create entries inside /home
# but prevents them from deleting or renaming entries owned by other users —
# safer than plain 0777 because it stops a compromised process from clobbering
# another user's home directory or symlinking it elsewhere.
RUN chmod 1777 /home

ENV KRONN_DATA_DIR=/data
ENV PATH="/home/kronn/.local/bin:/usr/local/bin:${PATH}"
EXPOSE 3140

USER kronn
ENTRYPOINT ["entrypoint.sh"]
CMD ["kronn"]
