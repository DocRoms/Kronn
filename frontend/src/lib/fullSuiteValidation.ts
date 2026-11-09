// KT-1636 — a validation line that replays the whole Rust suite. The PR's CI
// owns it; local integration validations are fmt + clippy + targeted lib tests.
// Mirrors `is_full_suite_validation` (backend/src/api/orchestration.rs), which
// raises the same `full_suite_ci_owned` warning at the MCP preflight.
const NARROWING = /^--(lib|bins?|tests?|doc|examples?|benches|bench|no-run|list)(=|$)/;
const VALUE_FLAGS = new Set([
  '--manifest-path', '--target-dir', '--config', '--features', '--package', '--profile',
  '--jobs', '--color', '--message-format', '--cargo-profile', '-p', '-j', '-F', '-Z',
]);

export function isFullSuiteValidation(command: string): boolean {
  const words = command.trim().split(/\s+/).filter(Boolean);
  if (words[0] === 'make') return words[1] === 'test-backend' || words[1] === 'test-backend-cov';
  let rest = words;
  if (rest[0] === 'rtk') rest = rest[1] === 'proxy' ? rest.slice(2) : rest.slice(1);
  if (rest[0] !== 'cargo') return false;
  const cargo = rest.slice(1);
  const sub = cargo.findIndex(word => !word.startsWith('-'));
  if (sub < 0) return false;
  let args = cargo.slice(sub + 1);
  if (cargo[sub] === 'nextest') {
    if (args[0] !== 'run') return false;
    args = args.slice(1);
  } else if (cargo[sub] !== 'test') {
    return false;
  }
  for (let i = 0; i < args.length; i += 1) {
    const word = args[i];
    if (word === '--') return !args.slice(i + 1).some(rest2 => !rest2.startsWith('-'));
    if (NARROWING.test(word)) return false;
    if (VALUE_FLAGS.has(word)) { i += 1; continue; }
    if (!word.startsWith('-')) return false;
  }
  return true;
}
