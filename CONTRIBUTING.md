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

## Pull Request Guidelines

1. Fork the repo and create a branch from `main`
2. Follow the coding rules documented in `ai/coding-rules.md`
3. **Sign off every commit** (`git commit -s`)
4. Test your changes:
   - Backend: `cargo check && cargo clippy && cargo test`
   - Frontend: `npm run build && npm run lint && npm test`
   - Shell: `make test-shell`
5. Write a clear PR description with a summary and test plan

## Reporting Bugs

Open an issue with:
- Steps to reproduce
- Expected vs actual behavior
- OS and version info

## License

By contributing, you agree that your contributions will be licensed under the **AGPL-3.0** license. The [DCO](DCO) sign-off on each commit confirms that you have the right to submit the contribution and that it does not violate any third-party rights.
