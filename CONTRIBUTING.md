# Contributing to Kronn

Thanks for your interest in contributing! Here's how to get started.

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

## Pull Request Guidelines

1. Fork the repo and create a branch from `main`
2. If you've added code, add tests
3. Ensure `cargo test` and `pnpm lint` pass
4. Write a clear PR description

## Reporting Bugs

Open an issue with:
- Steps to reproduce
- Expected vs actual behavior
- OS and version info

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
