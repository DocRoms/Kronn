You are running a FOCUSED SECURITY AUDIT (0.8.2). Output ONLY security findings — ignore unrelated bugs, perf issues, documentation drift, etc.

# A. MANDATORY SECURITY CHECKLIST (never skip, never trim)

Walk through every item below. If satisfied, write "verified present" or "verified absent". If NOT satisfied, emit a TD finding. These do NOT count against any cap.

**Secrets & credentials in source:**
- No REAL `.env*` file is tracked (`git ls-files -- '*.env*' '.env*'` — pathspec listing, no grep) — `.env.example`/`.env.dist`/`.env.sample` are legitimate templates and NOT findings. Tracking status and file NAMES are the ONLY safe signals you may collect.
- **Secret VALUES are out of your scope entirely.** You may not open, grep, or otherwise inspect file contents for secret-looking values — any command whose output could echo a matched line would pull the secret into your context, which leaves the machine. For the value-scan sub-dimension, write exactly `Not evaluated safely (requires Kronn's local secret scanner)` — NEVER `verified absent`/`verified clean` on a scan you did not safely run.
- CI YAML / docker-compose credential hygiene: `Not evaluated safely (requires Kronn's local secret scanner)` — proving the absence of a hardcoded credential means reading the very content that could contain it. You may only note the SAFE structural facts: which workflow/compose files exist, and whether they use `secrets:`/`env_file:` — established EXCLUSIVELY via `grep -lE 'secrets:|env_file:' <files>` (filename-only output, zero lines, zero context; `-l` is the ONLY grep flag allowed near credential material).
- No private keys or certs in repo (`*.pem`, `*.key`, `*.p12`, `id_rsa*`). A key often has no extension: list `**/*.pub` — for each `X.pub`, a tracked `X` beside it is the private half — and every file under a `.ssh/` directory. Report their PATHS only, never open them.
- `git log --all -S 'BEGIN PRIVATE KEY' --name-only --format='%h %ad'` returns nothing recent (last 12 months) — metadata only, NEVER `-p` (a patch dump would pull the key itself into your context).

**EXFILTRATION GUARD (absolute)**: you run with filesystem access and your context leaves the machine. NEVER open, quote, print or write the VALUE of any secret, key or token — in your output, in docs/, in TDs, anywhere. Findings carry file + line + pattern type only.

**Authentication & sessions:**
- Session cookies set with `HttpOnly` AND `Secure` AND `SameSite=Lax|Strict` (grep for `setcookie`, `Set-Cookie`, `withCredentials`).
- Password storage uses a slow hash (`bcrypt`, `argon2`, `scrypt`, `PBKDF2`) — not MD5/SHA1/plain.
- JWT secret sourcing and hardcoded `Bearer` tokens: `Not evaluated safely (requires Kronn's local secret scanner)` — same boundary, the match line would BE the secret. You may note the safe structural fact that a JWT/auth library is configured (dependency name from the manifest) without inspecting its key material.
- Password reset tokens are single-use AND time-limited.

**Input validation & injection:**
- SQL: every query uses parameterized binding (no raw string concat with user input). Grep for patterns like `query("..." + ` or `query(f"...{user_var}"` or PHP `".$user."`.
- Shell: no `exec`, `system`, `passthru`, `subprocess.run(..., shell=True)`, Rust `std::process::Command` spawning `sh`/`bash`/`cmd.exe` with `-c`/`/c` + user input.
- File paths: no user-controlled string flowing into `fs::open`, `open()`, `file_get_contents()` without `realpath`/`canonicalize` + allowlist root.
- Templates: no user input in `dangerouslySetInnerHTML`, `v-html`, `{!! $x !!}` (Blade), `{{ x | safe }}` (Jinja). Sample 3 templates.
- Deserialization: no `pickle.loads`, `unserialize`, `yaml.load` (without `SafeLoader`), `Marshal.load` on untrusted input.

**Transport & headers:**
- HTTPS enforced (nginx/middleware redirects HTTP → HTTPS, or `secure_origin_whitelist` is empty in prod).
- `Strict-Transport-Security` header set with `max-age >= 15768000` and `includeSubDomains`.
- `Content-Security-Policy` header set (even a permissive one with `default-src 'self'` is better than absent).
- `X-Frame-Options: DENY` or CSP `frame-ancestors 'none'` to block clickjacking.
- `X-Content-Type-Options: nosniff` set.
- CORS allowlist is explicit (not `Access-Control-Allow-Origin: *` for any endpoint that touches credentials).

**Dependencies & supply chain:**
- Lockfile present and tracked (`package-lock.json`, `pnpm-lock.yaml`, `composer.lock`, `Cargo.lock` for bins, `poetry.lock`, `Gemfile.lock`).
- Dependency risk signals you can VERIFY in the repo: lockfile pins to versions the project's own tooling flags (`cargo audit`/`npm audit`/`composer audit` output or config if present), deps several majors behind their manifest constraint, or abandoned forks. Do NOT assert specific CVEs from memory — model knowledge is stale; if no tooling output exists, record `Inferred` and suggest wiring an audit tool instead.
- Version gaps and EOL runtimes: judge them only from the `Technologies and standards` block below and cite its dated rows; otherwise write `not verified: no source`.
- CI runs a dependency audit step (`npm audit`, `pnpm audit`, `cargo audit`, `composer audit`, `pip-audit`, etc.) or has a Dependabot/Renovate config.
- No `npm install` / `pip install` of git URLs with `master`/`main` branches (must be tagged or pinned commit).

**SSH/CI deploy:**
- No `StrictHostKeyChecking=no` in deploy scripts.
- Deploy SSH keys have a clear rotation procedure documented (search for `rotat` keyword in deploy docs).
- `pull_request_target` workflows do NOT check out the PR branch's workflow definition (RCE vector).

# B. ANTI-REPETITION (priors)

Before writing findings: read existing `docs/tech-debt/TD-*-{auth,security,secrets,xss,sql,cors,csp,...}*.md`. If a finding you'd write matches an existing one, **REUSE its ID** (e.g. update the matching `TD-20260315-jwt-secret-hardcoded.md` in place — APPEND a new `audit_history` entry to the YAML frontmatter, never overwrite). A new audit run does NOT mean new slugs. Slug churn is the #1 audit anti-pattern.

# C. OUTPUT FORMAT

1. Write `docs/inconsistencies-security.md` with the standard table header:
`| ID | Problem | Area | Severity |`
2. For each finding, also write `docs/tech-debt/<ID>.md` using the existing TD detail template — add a `**Category**: security` line so a future Full-audit reconciliation can dedupe by category.
3. **Status taxonomy** (two-tier):
- `Verified in source` — you opened the source file and confirmed the issue still exists at the cited path.
- `Inferred` — pattern-matched only; the fix may already be in flight upstream.
Use the first one whenever you actually read source.
4. **Cap**: 25 findings max from § A. Critical and High are exempt — if your scan finds 6 Critical, you emit all 6 even if the cap is hit.
5. Each finding has: file path + line number, one-sentence impact (what an attacker gains), one-sentence remediation. No long essays — this is a triage list, not a security report.

Do NOT modify source code. Do NOT touch other docs/ files. Your scope is only the two output files above.