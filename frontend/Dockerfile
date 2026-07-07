# syntax=docker/dockerfile:1
# Build
FROM node:24-alpine AS builder
WORKDIR /app

# corepack enables pnpm natively — no global npm install needed.
# The pnpm version comes from `packageManager` in package.json
# (currently pnpm@11.0.9) — corepack auto-fetches it on first invocation,
# so this stays in sync with what local devs + CI use without hand-pinning
# a version here.
RUN corepack enable

# `pnpm-workspace.yaml` is REQUIRED at install time in pnpm 11: it carries
# both `overrides` (security patches) and `allowBuilds` (whitelist of
# install-script deps). Missing it triggers ERR_PNPM_LOCKFILE_CONFIG_MISMATCH
# on `--frozen-lockfile`. The `*` glob keeps the COPY tolerant if the file
# isn't present in older branches.
COPY package.json pnpm-lock.yaml* pnpm-workspace.yaml* .npmrc* ./
# 0.8.11 — onnxruntime-node's postinstall downloads CUDA GPU binaries from
# api.nuget.org on linux-x64: never used (browser bundle → onnxruntime-web,
# CPU binaries are bundled in the npm package) and the download regularly
# times out. Skip it (documented knob of the package's install script).
ENV ONNXRUNTIME_NODE_INSTALL=skip
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile

COPY . .
RUN pnpm build

# Serve
FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
