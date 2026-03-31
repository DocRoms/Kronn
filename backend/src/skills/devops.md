---
name: devops
description: Use when modifying CI/CD pipelines, Dockerfiles, infrastructure config, deployment scripts, or monitoring setup. Covers containers, IaC, observability, and cost.
license: AGPL-3.0
category: domain
icon: 🚀
builtin: true
---

## Procedures

1. **Dockerfile** — multi-stage build, pin base image digest (not `latest`), run as non-root, use `.dockerignore`.
2. **CI pipeline** — fail fast (lint → test → build). Cache dependencies. Keep under 10 min for PR feedback.
3. **IaC** — Terraform/Pulumi in version control. Plan in CI, apply only from CI. Never manual console changes.
4. **Monitoring** — metrics (Prometheus), logs (structured JSON), traces (OpenTelemetry). Define SLOs before alerts.
5. **Deploy** — blue-green or canary. Rollback plan before every deploy. Health checks on readiness + liveness.

## Gotchas

- **Docker layer cache** invalidates on ANY changed line and all lines after — put `COPY package.json` before `COPY .` to cache deps.
- **GitHub Actions `latest`** on actions is dangerous — pin to SHA: `uses: actions/checkout@<sha>`.
- **Secrets in CI** — never echo/print env vars in logs. Mask with `::add-mask::`. Prefer OIDC over long-lived tokens.
- **Kubernetes is overhead** — for <5 services, Docker Compose or ECS is usually enough. Don't adopt K8s for resume-driven development.
- **Spot/preemptible instances** save 60-80% but need graceful shutdown handling (SIGTERM → drain → exit).
- **12-factor: config in env** — but never write env vars to disk. Use secret managers for anything sensitive.

## Validation

- Dockerfile builds in CI with no warnings. Image size is reasonable (<500MB for most apps).
- Pipeline runs under 10 min. No `latest` tags on base images or actions.
- All secrets come from vault/SSM, none hardcoded.

✓ Multi-stage Dockerfile, non-root, pinned base image, cached dependency layer.
✗ `FROM node:latest`, runs as root, `COPY . .` as first step.
