# Contributing to Kronn

Thanks for your interest in contributing! This project is licensed under the [GNU Affero General Public License v3.0 (AGPL-3.0)](LICENSE).

## What does AGPL-3.0 mean?

The AGPL-3.0 is a strong copyleft license:

- **Freedom to use**: Anyone can use, modify, and distribute Kronn.
- **Network clause**: If you modify Kronn and deploy it as a service (even without distributing the binary), you **must** make your modified source code available under AGPL-3.0.
- **Copyleft**: All derivative works must also be licensed under AGPL-3.0.
- **Attribution**: You must retain copyright notices and license headers.

This ensures that improvements to Kronn always benefit the community.

## Development Setup

```bash
# Prerequisites
make check

# Start backend with hot reload
make dev-backend

# Start frontend dev server (separate terminal)
make dev-frontend
```

## Project Structure

- **backend/** — Rust (Axum). All types in `models/mod.rs` are the single source of truth.
- **frontend/** — React + TypeScript. Types are auto-generated from Rust via `ts-rs`.

Run `make typegen` after modifying any Rust model to regenerate `frontend/src/types/generated.ts`.

## Refreshing README screenshots

If your change touches the UI shown in the README (Projects dashboard, Quick Prompts tab, QP launch form, workflow wizard), reshoot the affected screenshots from the screenshot sandbox so the maintainer's real data never leaks into docs.

See [`docs/operations/screenshot-sandbox.md`](docs/operations/screenshot-sandbox.md) for the workflow.

## Developer Certificate of Origin (DCO)

This project uses the [DCO](DCO). Every commit **must** be signed off to certify that you have the right to submit it under the AGPL-3.0 license.

### How to sign off

Add `-s` (or `--signoff`) to your commit command:

```bash
git commit -s -m "feat: my contribution"
```

This adds a `Signed-off-by` line to your commit message:

```
feat: my contribution

Signed-off-by: Your Name <your@email.com>
```

The name and email come from your git config (`user.name` and `user.email`). Use your **real name** (no pseudonyms) — the DCO is a legal declaration.

### Forgot to sign off?

```bash
# Amend the last commit
git commit --amend -s

# Sign off multiple past commits (e.g. last 3)
git rebase --signoff HEAD~3
```

### AI-assisted contributions

Commits authored or co-authored by AI tools (Claude Code, Copilot, etc.) must still carry a human `Signed-off-by`. The human is responsible for reviewing and certifying the contribution.

## AI Context (`ai/`)

This project uses an AI-optimized context system in `ai/`. Before making changes:

1. Read `ai/index.md` — it contains project rules, coding conventions, and architecture decisions.
2. After completing your work, update the relevant `ai/` file if you discovered something non-obvious (a gotcha, a pattern, an outdated doc).
3. **Do not confuse** `ai/` (context for agents working on THIS repo) with `templates/ai/` (template skeleton installed into projects managed by Kronn).

## Pull Request Guidelines

1. Fork the repo and create a branch from `main`
2. Follow the coding rules documented in `ai/coding-rules.md`
3. **Sign off every commit** (`git commit -s`)
4. Test your changes:
   - Backend: `make test-backend` (cargo-nextest, the runner CI uses; `make install-dev-tools` installs it), `cd backend && cargo clippy --all-targets -- -D warnings`, and `make test-backend-cov` for the coverage floors
   - Frontend: `pnpm build && pnpm lint && pnpm test`
   - Shell: `make test-shell`
   - E2E (Playwright, optional but recommended for UI changes): `make test-e2e` — requires the backend running. See [`frontend/e2e/README.md`](frontend/e2e/README.md) for the full setup + how to add a spec.
5. Write a clear PR description with a summary and test plan

## CI labels

Pull-request CI runs under two labels:

- **`ci-test`**: the fast loop, re-run on every push (formatting, clippy, the
  backend suite and its coverage floors, browser E2E, frontend, shell, Python,
  security, duplication and diff hygiene). `ci-quality-gates` is its required
  check.
- **`ci-build`**: the app builds (the backend in the real release profile,
  macOS and Windows portability tests, the desktop crate, the Windows document
  exporter). Required before merge: add it once the pull request is ready.
  `ci-build-gates` is its required check and stays red without the label.

Both labels persist: opening, reopening and every push (`synchronize`) run
**CI Tests** (`ci-test.yml`) and **CI Build** (`ci-build.yml`) for the new
head. Their jobs read the labels live, so a job skipped there means the label
was missing, never that it ran elsewhere.

Labels and edits (title, description, base) start only **CI Verdict**
(`ci-verdict.yml`), which publishes nothing but the two aggregates. Each one
repeats the verdict CI Tests or CI Build gave for the same pull request, head
and base. When that run predates the label (you added `ci-test` or `ci-build`
after pushing), CI Verdict re-runs it once and repeats the new verdict; the
real job results then replace the skipped ones on the same run. On a fork,
whose token cannot re-run, a maintainer uses "Re-run all jobs" on that run.
After a change of base, push a commit or close and reopen the pull request.
Pushes to `main` and release tags run both test workflows.

Which checks matter: `ci-quality-gates` and `ci-build-gates`. They report on
every pull-request event, from whichever workflow ran last, and are the
verdicts to read; the individual jobs are the details.

`main` is protected by repository ruleset 13870406, whose required checks are
job names. A job skipped by its `if` reports as passing, so the job names
moved to `ci-build.yml` (such as `test-docs-sidecar-windows`) enforce nothing
without the label. Only the two aggregates enforce their workflow: they always
run, and `ci-build-gates` fails while the `ci-build` label is missing. The app
builds are therefore required before merge only once a maintainer adds both
aggregates to the ruleset (repository settings, not code): read the ruleset,
append `{"context": "ci-quality-gates", "integration_id": 15368}` and
`{"context": "ci-build-gates", "integration_id": 15368}` to the
`required_status_checks` rule's list, keep every other field, then:

```bash
gh api repos/DocRoms/Kronn/rulesets/13870406 \
  | jq '{name, target, enforcement, conditions, bypass_actors, rules: [.rules[]
        | if .type == "required_status_checks" then .parameters.required_status_checks
            += [{context: "ci-quality-gates", integration_id: 15368},
                {context: "ci-build-gates", integration_id: 15368}] else . end]}' \
  > ruleset.json
gh api -X PUT repos/DocRoms/Kronn/rulesets/13870406 --input ruleset.json
```

The ruleset already requires branches to be up to date, so a base branch that
only advances (no pull-request event) still forces a new push, which re-runs
both test workflows.

## Reporting Bugs

Open an issue with:
- Steps to reproduce
- Expected vs actual behavior
- OS and version info

## License

By contributing, you agree that your contributions will be licensed under the **AGPL-3.0** license. The [DCO](DCO) sign-off on each commit confirms that you have the right to submit the contribution and that it does not violate any third-party rights.
