Real issues only, not hypothetical. Read current docs guidance (exclude dated `docs/reports/` snapshots) AND scan source code: entry points, config files, Dockerfiles, CI configs and 5-10 core source files (auth, data persistence, external input first).

# A. Baseline

Kronn already ran the mechanical baseline checks as detectors: see the `Deterministic detector signals` block below. Dispose of every signal (a TD, a baseline note, or a justified matrix row); a validator re-runs this step if one is left unaddressed. Detectors cover: Dockerfile (non-root `USER`, `HEALTHCHECK`, apt `--no-install-recommends` and list cleanup, base image untagged or `latest`, `ADD <remote-url>`, secret-named `ARG`), compose `image:` untagged or `latest`, CI (`StrictHostKeyChecking=no`, `pull_request_target`, actions on a moving ref), tracked `.env` files, templates (`<img>` without `alt`, inline `<script>`/`<style>` over 50 lines), and the two blocks below. Stack-specific items (PHP, Node.js, Rust, Python, Go) arrive in the `Stack-specific checklist` block, only for the stacks detected. Baseline findings do not count against the cap (§ D 5). Say when an item holds ("verified present", "verified absent"). Never close a finding on a word: resolved, mitigated, false alarm or not applicable needs `[src: file: path:line]` on the code in the same row or sheet, fallbacks read too (an `ssh-keyscan` fallback is not "verified correct"); else keep it open.

Still yours to judge, when the file type exists:
- **Compose**: resource limits (`mem_limit`/`cpus` or `deploy.resources.limits.*`) on services that can run away (app, workers, DB): `Medium` at most, and no finding when the file is dev-only or the platform owns limits (say which). `read_only: true` on clearly stateless services. Credential hygiene of `environment:` blocks: `Not evaluated safely (requires Kronn's local secret scanner)`; safe signal only: `grep -lE 'secrets:|env_file:' <compose files>` (filename-only).
- **CI**: a deploy workflow runs a quality gate (lint, test or static analysis) before production. CI secret sourcing: `Not evaluated safely (requires Kronn's local secret scanner)`; safe signal only: `grep -l 'secrets.' <workflow files>` (filename-only).
- **`.env*`**: Values inside tracked `.env*` files: `Not evaluated safely (requires Kronn's local secret scanner)`: never open or pattern-scan them; the `tracked-dotenv` detector judges filenames only. The deploy step excludes `.env*` from the bundle (rsync/scp/tar `--exclude`).
- **Web** (more than 3 templates): form inputs carry a `<label>` or `aria-label` (sample 3-5 templates); a Content-Security-Policy header is set where user input is handled.

**Community standards**, gated on OSS intent: the `oss-community-standards` detector opens the gate (a root `LICENSE`, a public remote, or a README inviting contributions) and lists what is missing among `LICENSE` (recognized body), a README description, `.github/ISSUE_TEMPLATE`, `pull_request_template.md`, `CONTRIBUTING.md` and `CODE_OF_CONDUCT.md`; `SECURITY.md` comes from `missing-community-files`. Low or Medium: project health, not security. Private projects get no signal and no TD.

**Repository hygiene**, always: the `tracked-repo-artifacts` detector reads `git ls-files --cached`, so only tracked/index content is judged, never harmless local/untracked developer files; it spares test fixtures and a documented Yarn zero-install/offline-mirror setup. Detector evidence is bounded to 15 paths per detector plus an explicit remainder count. A finding that a file merely exists or is tracked (a private key, a log) is cited `[src: file: <path>:exists]`, with no line and without quoting its content; a claim about what a text file contains still needs `[src: file: <path>:<line>]`.

# B. Dimensional scan

Account for EACH of the 10 dimensions in the `## Dimension coverage` matrix of the index, one outcome per row:
- **findings**: one or more TDs with `[src: file:…]` evidence;
- **scanned — nothing substantiable**: you opened the relevant code and found nothing to anchor (not "I didn't look");
- **N/A: <verifiable reason>**: a reason a human can check ("no Dockerfile", "no DB/ORM layer", "no web surface"), never "not relevant".
A missing row, a short matrix or an unverifiable reason is an incomplete audit: the validator fails and re-runs this step. On a non-trivial codebase, "scanned — nothing substantiable" deserves a second look before you write it.

- **Dependencies**: EOL/deprecated runtimes, frameworks, packages, or versions significantly behind stable — judged ONLY from the dated sources of the `Technologies and standards` block below, never from memory; the row's evidence states `source used: …` (or `source: …`, or a `[src: deps: …]` citation from the dependency watch table) or `not verified: no source`.
- **Security**: hardcoded secrets (regex: API keys, tokens, OAuth client_secret), missing auth checks, injection (SQL/XSS/command), insecure defaults, exposed debug endpoints, default credentials.
- **Code quality**: functions over 50 lines, files over 500 lines, god classes, dead code, swallowed errors (empty catch, `let _ =`, `unwrap_or_default` on a Result).
- **Scalability**: N+1 queries, unbounded loops, missing pagination, memory leaks, missing indexes on hot queries.
- **Maintainability**: tight coupling, circular dependencies, untested critical paths, unclear naming, mixed languages in comments.
- **Accessibility (a11y)**: the web baseline; for libraries/CLIs, machine-readable output and a quiet mode for scripts.
- **Observability**: logging on hot paths (auth, payment, writes), an error tracker, health/readiness endpoints, metrics on critical SLIs.
- **Compliance**: GDPR (external resources, data retention), license incompatibilities (`composer licenses` / `cargo deny` / `license-checker`).
- **Performance** (if perf-sensitive per `docs/briefing.md` or the README): CWV risk, bundle size, image optimisation, cache headers, CDN.
- **Documentation drift**: concrete contradictions between the docs written in steps 1-7 and the code (a rule claimed enforced with no linter, a wrong test count, a dead path in `repo-map.md`).

# C. Anti-repetition

List `docs/tech-debt/` (skip README.md, TEMPLATE.md, _template.md and files starting with `_`): these are earlier TDs. For each finding:
1. **Same root cause** as an existing TD (same file and anti-pattern, whatever the date in the slug): REUSE its ID and update the detail file in place (refresh `Where`, append an `audit_history` entry). Never a new `TD-YYYYMMDD-` file for it: the user may already have confirmed the old one.
2. **Refined** (same area, sharper scope or new evidence): reuse the ID, update title and description, note `"refined from previous audit"` in the history entry.
3. **Genuinely new**: a new `TD-YYYYMMDD-slug.md` dated today.
4. **Earlier TDs you did not re-emit**: list them under a `## Reconciliation candidates` section with your best guess (`fixed in commit X`, `no longer visible in source`, `out of scope this audit`, `uncertain`); a reconciliation pass runs after Step 8.
5. **Decisions are not debt**: read `docs/decisions.md` before emitting. A finding that an `accepted` ADR explains (e.g. CI triggered by a label as a budget guard) is a recorded choice: do NOT write a TD for it. Never add or change a `Decision record` line on a TD sheet: Kronn writes it when it settles one, and a line written during the run is ignored. A `proposed` or `superseded` ADR covers nothing. Kronn settles every TD sheet an accepted ADR covers after the steps.

# D. Output

Fill `docs/inconsistencies-tech-debt.md`: replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comments.
1. **Outdated prerequisites** table: EOL/deprecated/behind-stable runtimes, frameworks and packages, with the reality column and its dated source citation (or `not verified: no source`).
2. **Per issue**: create or update `docs/tech-debt/TD-<date>-<slug>.md` first, then add its one-line row to the `Current list` table. Both or neither; an update keeps the original file name and date. Point at a sheet with a link `[TD-x](tech-debt/TD-x.md)` or `[src: file: docs/tech-debt/TD-x.md:exists]`, never a bare sheet path in a `[src: file:]` marker.
3. **Severity**: Critical = production down, data leak, exploitable flaw (prod API key in the repo, SQL injection on a public endpoint). High = blocks a release or a supported environment, or a baseline failure touching production safety (root container, secret in a build `ARG`). Medium = daily friction or a measurable perf hit (slow suite, N+1 on a hot page, missing CI gate). Low = cosmetic (naming, missing doc comment, unused dependency).
4. **Status**: `Verified in source` (you read the code and cite file:line; the default for baseline and most dimensional findings, which validation skips), `Inferred` (from docs or briefing only; the user confirms), `Blocked upstream`, `Mitigated`. Only the validation phase sets `Confirmed by user`, `Rejected` (never recreated), `Deferred` and `Accepted decision` (Kronn too, when an accepted ADR covers the TD).
5. **Cap**: 30 findings. Critical and High, baseline included, are never trimmed; trim Low, then Medium, and note the trimmed Medium count under `## Coverage gaps`.
6. **Tracker dedup**: with a tracker MCP (Jira/Linear/GitHub Issues), search read-only for an open ticket before creating a NEW TD; if one matches, `Next step: link existing ticket <URL>`.
7. **No issues**: a single row `'None identified during initial audit'` (rare: even a green-field repo has baseline gaps).
8. **`## Dimension coverage`**: all 10 rows, each with an outcome per § B and an evidence/reason cell. Create the section if the index predates it.

**Detail file format** (YAML frontmatter + markdown sections, enforce this shape):
```
---
name: td-<date>-<slug>
description: One-line summary (≤ 160 chars).
metadata:
type: tech-debt
audit_history:
- date: YYYY-MM-DD
status: Verified in source | Inferred | Blocked upstream | Mitigated | Confirmed by user | Rejected | Accepted decision | Deferred
reviewer: <audit kind, e.g. "Full audit" or "Security audit">
note: optional — what changed since previous entry (e.g. "line numbers shifted", "severity bumped to High")
---

# TD-<date>-<slug>

- **Area**: Backend | Frontend | CI | Infra | Security | A11y | Observability | Database | ApiDesign | Docs | Other
- **Severity**: Critical | High | Medium | Low
- **Status**: <current value, mirrors latest audit_history entry>
- **Effort**: S (< 1h) | M (1-4h) | L (1+ day) | XL (multi-day / cross-team)
- **Blast radius**: local (1-2 files) | module (5-10 files) | cross-cutting (50+ files or new pattern)

## Problem (fact)
<one or two sentences, factual — what is, not what should be>

## Impact
<what goes wrong if not fixed — concrete>

## Where (pointers)
- `path/to/file.rs:42` — <one-line context>

## Suggested direction
<non-binding fix suggestion: a skeleton, NOT a full implementation>

## Next step
`create ticket` OR `link existing ticket <URL>` (see point 6).
```

For updates, APPEND to `audit_history`; never replace earlier entries.

**Scope reminder**: this step ONLY fills `docs/inconsistencies-tech-debt.md` and creates/updates `docs/tech-debt/TD-*.md` detail files. `docs/decisions.md` is filled in Step 9, once the whole audit picture exists.