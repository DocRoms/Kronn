#!/usr/bin/env bash
# ── Scoped coverage floor: encryption-key & config-persistence modules ────────
# These files are the 2026-06-30 secret-loss hardening (the encryption key must
# never be silently lost). They must stay near-fully covered so a regression
# that could orphan the key can't merge unnoticed. This gate is SEPARATE from
# and stricter than the repo-wide floor — it does not replace it.
#
# Floors are pinned just below the current measured region/line coverage. When a
# push raises coverage, RAISE these in lockstep — never lower them, that defeats
# the purpose. Values that are intentionally NOT 100%: `keyvault.rs` (the real
# OsKeychain backend can't run in headless CI — mocked in unit tests, gated out
# here), and non-unix `#[cfg]` branches in `config.rs`.
set -euo pipefail

cd "$(dirname "$0")/../backend"

# "file-substring : min-region-% : min-line-%"
TARGETS=(
  "core/crypto.rs:98:98"
  "core/config.rs:93:96"
  "core/keyvault.rs:85:87"
  "core/keystore.rs:89:91"
  "core/recovery.rs:92:94"
  "db/mcps.rs:89:91"
  "db/migrations.rs:92:94"
)

# Reuse the instrumented build/profdata from the repo-wide floor step if it ran;
# otherwise produce it here. --ignore-run-fail so pre-existing env-flaky tests
# (git-worktree / mcp_scanner parallelism) don't abort the coverage report.
json="$(cargo llvm-cov report --json 2>/dev/null \
        || cargo llvm-cov --lib --ignore-run-fail --json)"

fail=0
for entry in "${TARGETS[@]}"; do
  IFS=':' read -r frag min_reg min_line <<< "$entry"
  # Note: read from a here-string (not process substitution) so an empty match
  # doesn't trip `set -e` via read's EOF non-zero exit.
  vals="$(printf '%s' "$json" | jq -r --arg f "$frag" '
    .data[0].files[] | select(.filename | contains($f))
    | "\(.summary.regions.percent) \(.summary.lines.percent)"' | head -1)"
  reg=""; line=""
  [[ -n "$vals" ]] && read -r reg line <<< "$vals"
  if [[ -z "${reg:-}" ]]; then
    echo "✗ ${frag}: not found in coverage report"; fail=1; continue
  fi
  reg_i=${reg%.*}; line_i=${line%.*}
  if (( reg_i < min_reg )) || (( line_i < min_line )); then
    printf '✗ %-20s regions %s%% (min %s) · lines %s%% (min %s)\n' "$frag" "$reg" "$min_reg" "$line" "$min_line"
    fail=1
  else
    printf '✓ %-20s regions %s%% · lines %s%%\n' "$frag" "$reg" "$line"
  fi
done

if (( fail != 0 )); then
  echo "Key-management coverage floor breached — add tests, don't lower the floor."
  exit 1
fi
echo "Key-management coverage floor OK."
