import { describe, expect, it } from 'vitest';
import { isFullSuiteValidation } from '../fullSuiteValidation';

// KT-1636 — mirrors the backend `is_full_suite_validation` cases.
describe('isFullSuiteValidation', () => {
  it.each([
    'cargo test',
    'cargo test --no-fail-fast',
    'cargo test --manifest-path backend/Cargo.toml --target-dir /tmp/t',
    'cargo test --workspace -- --nocapture',
    'rtk cargo test',
    'cargo nextest run --workspace',
    'cargo --offline test',
    'make test-backend',
    'make test-backend-cov',
  ])('flags `%s` as the full suite', command => {
    expect(isFullSuiteValidation(command)).toBe(true);
  });

  it.each([
    'cargo fmt --check',
    'cargo clippy --all-targets -- -D warnings',
    'cargo test --lib',
    'cargo test --lib --manifest-path backend/Cargo.toml orchestration::',
    'cargo test leading_thinking_filter',
    'cargo test --test api_integration',
    'cargo test --doc',
    'cargo test --no-run',
    'cargo test -- reaper',
    'cargo nextest run --lib',
    'cargo nextest list',
    'make test-backend-lib',
    'pnpm --dir frontend exec tsc -b',
    '',
  ])('does not flag `%s`', command => {
    expect(isFullSuiteValidation(command)).toBe(false);
  });
});
