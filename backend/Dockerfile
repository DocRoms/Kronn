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
# Embedded resources read by `include_str!` at compile time (e.g. the Phase 2
# anti-hallu spec in `core::anti_halluc::SPEC_AGENTS_MD_V1`). Anything under
# `backend/` referenced via `include_str!` MUST also be COPIED here, otherwise
# the Docker builder breaks even though `cargo build` works locally. Caught
# 2026-05-26 when shipping the spec doc — see feedback_include_str_docker_context.
COPY docs ./docs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    # 0.8.6 fix : `cargo clean -p kronn` BEFORE the build nukes the stale
    # `kronn` artifact left over by the dummy-lib first pass (line 17).
    # Without this, cargo's incremental cache keeps the empty-lib `.rlib`
    # metadata and the bin's `kronn::core` / `kronn::db` / … imports fail
    # with E0432. Cleaning only the `kronn` package preserves all deps in
    # the BuildKit cache → no extra dep-rebuild cost. Caught 2026-05-20
    # after adding `agent_api.rs` + `TokenExchange` variant.
    cargo clean -p kronn 2>/dev/null || true \
    && touch src/main.rs src/lib.rs \
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
    ca-certificates git curl nodejs npm python3 python3-pip python3-venv \
    make gh openssh-client unzip \
    # kronn-docs sidecar (WeasyPrint) runtime libs. WeasyPrint >=62 renders via
    # Pango + HarfBuzz; the older cairo/gdk-pixbuf libs are kept as a harmless
    # superset so any pinned version in the <65 range resolves. fonts-dejavu-core
    # gives WeasyPrint a default font so PDFs aren't glyphless.
    libpango-1.0-0 libpangoft2-1.0-0 libharfbuzz-subset0 \
    libgdk-pixbuf-2.0-0 libcairo2 libffi8 fonts-dejavu-core libfontconfig1 \
    && rm -rf /var/lib/apt/lists/*

# ── Playwright / Chromium for agent browser tooling ────────────────────────
# Agents drive a REAL browser through the Playwright MCP (inspect prod pages,
# verify runtime behaviour — e.g. is an Adobe opt-in object present). This slim
# runtime image ships none of Chromium's system libs (libnss3, libgbm1, the
# GTK/X11/alsa stack), and the non-root `kronn` agent has no working sudo/apt —
# so `mcp__Playwright__browser_navigate` failed with "missing libraries" and the
# browser could never launch (only the host CLI, with a full desktop, worked).
# Bake the browser AND its deps into the image, in a shared world-rwx path that
# the kronn-run MCP both reads from and (if its pinned build differs) can refresh.
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
# Two distinct things: (1) the apt system libs Chromium needs (version-
# independent), and (2) the EXACT browser the Playwright MCP drives. The MCP
# (even invoked as `--browser chromium`) uses the **chrome-for-testing** build,
# a different artifact than `playwright install chromium` — so we install THAT
# one via the MCP's own command. Without it, `browser_navigate` returned
# "Browser chrome-for-testing is not installed" and the agent's browser tooling
# never worked. World-rwx so the kronn agent reads it (and can refresh if a
# newer MCP pins a different build).
RUN npx -y playwright@latest install-deps chromium \
    && npx -y @playwright/mcp@latest install-browser chrome-for-testing \
    && chmod -R a+rwx /ms-playwright \
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

# Install RTK (Rust Token Killer) — compresses shell outputs from agents
# before they reach the LLM context. Pinned: RTK is 0.x, each bump is
# manual + validated against regression fixtures in backend/tests.
ARG RTK_VERSION=0.39.0
RUN ARCH=$(dpkg --print-architecture) && \
    case "$ARCH" in \
      amd64) RTK_TARGET=x86_64-unknown-linux-musl ;; \
      arm64) RTK_TARGET=aarch64-unknown-linux-gnu ;; \
      *) echo "Unsupported arch for RTK: $ARCH" && exit 1 ;; \
    esac && \
    curl -fsSL -o /tmp/rtk.tar.gz "https://github.com/rtk-ai/rtk/releases/download/v${RTK_VERSION}/rtk-${RTK_TARGET}.tar.gz" && \
    tar -xzf /tmp/rtk.tar.gz -C /usr/local/bin rtk && \
    chmod +x /usr/local/bin/rtk && \
    rm /tmp/rtk.tar.gz

# Install ccusage (agent CLI usage/cost reporter) globally so `GET /api/usage`
# can shell out to it with zero runtime npx fetch — same "pre-installed, pinned"
# discipline as RTK above. Reads the CLIs' local JSONL logs (Claude / Codex /
# Gemini …) and reports real token + cache breakdown with up-to-date pricing.
ARG CCUSAGE_VERSION=20.0.5
RUN npm install -g ccusage@${CCUSAGE_VERSION} && npm cache clean --force \
    # ccusage 20.x ships a native binary it tries to `chmod +x` on first run.
    # The backend runs as the non-root `kronn` user, who can't chmod a
    # root-owned file → EPERM at runtime. Pre-make it executable AND hand the
    # ccusage tree to kronn so any runtime self-heal chmod is permitted.
    && chmod -R a+rx /usr/local/lib/node_modules/ccusage \
    && chown -R kronn:kronn /usr/local/lib/node_modules/ccusage

# ── kronn-docs sidecar (PDF/DOCX/XLSX/PPTX generation) ──────────────────────
# The backend spawns a Python sidecar (FastAPI + WeasyPrint/python-docx/…) from
# `~/.kronn/venv/docs` to generate downloadable documents. Outside Docker that
# venv is created by `make docs-setup`; inside the image we BAKE it so document
# generation works out of the box — no manual step, no "Document sidecar
# unavailable" 503. `~/.kronn/venv` is NOT a mounted volume (compose only mounts
# the `~/.kronn/user-context` subdir), so the baked venv survives at runtime,
# where $HOME=/home/kronn → `venv_python_path()` resolves to it exactly.
COPY sidecars/docs /app/sidecars/docs
RUN python3 -m venv /home/kronn/.kronn/venv/docs \
    && /home/kronn/.kronn/venv/docs/bin/pip install --no-cache-dir --upgrade pip \
    && /home/kronn/.kronn/venv/docs/bin/pip install --no-cache-dir /app/sidecars/docs \
    && chown -R kronn:kronn /home/kronn/.kronn

COPY --from=builder /usr/local/bin/kronn /usr/local/bin/kronn
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
COPY scripts/ /app/scripts/

# Data directory owned by kronn
RUN mkdir -p /data && chown kronn:kronn /data

# Pre-create directories that volumes will mount into
RUN mkdir -p /home/kronn/.local/bin /home/kronn/.local/share/uv \
    /home/kronn/.npm /home/kronn/.cache/uv /home/kronn/.config \
    && chown -R kronn:kronn /home/kronn/.local /home/kronn/.npm /home/kronn/.cache /home/kronn/.config

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
# The container runs as the non-root `kronn` user, but npm's default global
# prefix (/usr/local) is root-owned → `npm install -g <agent>` fails with
# EACCES, both at boot (entrypoint) and from the "Install" button. Point the
# global prefix at the user-writable, already-on-PATH ~/.local so every
# `npm install -g` lands in /home/kronn/.local/bin. Declared AFTER the
# build-time `npm install -g ccusage` above so that one still uses /usr/local.
# Container-scoped only — the Tauri/host build never sees this Dockerfile.
ENV NPM_CONFIG_PREFIX=/home/kronn/.local
EXPOSE 3140

USER kronn
ENTRYPOINT ["entrypoint.sh"]
CMD ["kronn"]




