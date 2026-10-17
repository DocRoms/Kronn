You are running a FOCUSED DOCKER & CONTAINER AUDIT (0.8.2). Output ONLY container/image/orchestration findings — ignore unrelated bugs.

# A. DOCKERFILE CHECKLIST (per Dockerfile in repo — root + subdirs)

For EACH `Dockerfile`, `*.dockerfile`, or `*/Dockerfile` (use `git ls-files | grep -i dockerfile`):
- **Base image is pinned** to a specific tag (not `:latest`, not just `python` — must be `python:3.11.7-slim` or a digest).
- **`USER <non-root>` present** before the `CMD`/`ENTRYPOINT` (root inside a container is a privilege-escalation jump pad).
- **`HEALTHCHECK` directive present** (or documented absent because the orchestrator handles it).
- **Layer hygiene**: `apt-get install` chains `--no-install-recommends && rm -rf /var/lib/apt/lists/*` in the same RUN. Equivalent for `yum`, `apk`, `dnf`.
- **No secrets via `ARG`** (they leak via `docker history`). Multi-stage `--mount=type=secret` is fine.
- **No `ADD <remote-url>`** without checksum verification — prefer `RUN curl ... && sha256sum -c`.
- **Multi-stage** used when the final image carries a build toolchain (Node, Cargo, Go, JDK) it doesn't need at runtime.
- **`COPY --chown`** used when copying app files (otherwise root-owned files under a non-root USER).
- **`.dockerignore` present** at the directory the Dockerfile sits in, excluding at minimum `.git`, `node_modules`, `target`, `vendor`, `.env*`.
- **PHP-specific** (if the image runs PHP): `display_errors = Off`, `expose_php = Off`, `opcache.enable = 1` for prod; verify `php.ini`/`*.ini` overrides.
- **Node-specific**: `NODE_ENV=production` set, no `npm install` (must be `npm ci` for reproducibility).

# B. COMPOSE CHECKLIST (per `docker-compose*.yml` / `compose*.yaml`)

- **Resource limits**: services that can realistically starve the host (app, workers, DB) carry `mem_limit`/`cpus` or `deploy.resources.limits.*`. Not a finding on clearly dev-only compose files or when the deployment platform owns limits — state which case applies.
- **No `:latest`** image tags (reproducibility).
- **Healthchecks**: critical services (DB, app, gateway) have `healthcheck:` blocks AND `depends_on` uses `condition: service_healthy` instead of bare `depends_on: [svc]`.
- **Secrets handling**: `Not evaluated safely (requires Kronn's local secret scanner)` — never grep for credential literals (the match line would be the secret). Safe signal: `grep -lE 'secrets:|env_file:'` (filename-only) shows whether the safe mechanisms are present.
- **Read-only rootfs**: stateless services have `read_only: true` (or note exceptions). Worth a TD only when the service is obviously stateless (gateway, static-asset proxy).
- **Restart policy**: services have `restart: unless-stopped` or `restart: always` (otherwise a crash stops them silently).
- **Logging cap**: `logging.options.max-size` + `max-file` set somewhere — either per-service or via a default driver — otherwise the host disk fills up.
- **Networks**: services that don't need each other use separate networks (defense-in-depth — emit TD only for obvious cases like a DB sharing the same network as an outbound-facing proxy).
- **Volume mounts**: no `${HOME}:/host-home:rw` style host-root mounts on a service that doesn't need them. Read-only is OK; rw on `/`, `/home`, `/var` is a red flag.

# C. CI/IMAGE-BUILD CHECKLIST

- CI step that builds the image runs a vulnerability scanner (`trivy`, `grype`, `snyk container test`) OR a TD is filed acknowledging the gap.
- Image push uses a tag derived from git sha or release version, never just `:latest` alone (multi-tag with `:latest` is fine).
- Cache layers: COPY commands ordered cheapest-changing → most-changing (lockfiles before source).

# D. ANTI-REPETITION (priors)

Before writing findings: read existing `docs/tech-debt/TD-*-{docker,compose,dockerfile,image,layer,...}*.md`. If a finding matches, reuse its ID (APPEND an `audit_history` entry to the YAML frontmatter — do not overwrite). Slug churn is the #1 audit anti-pattern.

# E. OUTPUT FORMAT

1. Write `docs/inconsistencies-docker.md` with header `| ID | Problem | Area | Severity |`.
2. For each finding, write `docs/tech-debt/<ID>.md` with a `**Category**: docker` line in addition to the standard TD schema.
3. **Status taxonomy** (two-tier): `Verified in source` when you opened the file and confirmed; `Inferred` for pattern-match only.
4. **Cap**: 25 findings max. Critical/High exempt.
5. Each finding: file path + line number, one-sentence impact (outage/security/perf cost), one-sentence remediation.

Do NOT modify source. Do NOT touch other docs/ files.