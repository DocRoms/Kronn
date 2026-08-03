# 0.9.3 maintenance baseline and outcome

Captured on 2026-08-02. The first section archives the pre-maintenance
snapshot; the following sections record the validated outcome and remaining
release gates.

## Archived baseline

- Frontend lint: 161 warnings and 0 errors on 2026-07-30.
- Frontend security: 23 advisories (8 high, 10 moderate, 5 low).
- Backend compatible lock refresh: 112 packages reported by the dry run.
- Desktop compatible lock refresh: 148 packages reported by the dry run.
- TypeScript 7 and Oxlint had not been measured on Kronn; the first pass used
  the last native preview available before the stable TypeScript 7 release.

The frontend warning debt was subsequently reduced contextually to 92 without
disabling the authoritative ESLint rules. Details and the remaining inventory
live in the dedicated React debt entry. [src: file:
docs/tech-debt/TD-20260509-react19-effect-rules.md:20-43]

## Dependency and security outcome

### Backend Rust

The compatible backend lock refresh updated 113 packages. The following gates
pass:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` (4,701 passed, 4 ignored)
- `cargo audit`

The final audit reports no vulnerabilities. Its three informational warnings
concern the unmaintained `backoff` and `instant` crates and a `ttf-parser`
soundness warning in the current transitive graph; none breaches the existing
audit policy.

### Desktop and Tauri

The desktop lock was refreshed independently from the embedded backend. The
final compatible delta includes `ipnet 2.12.1` and `time 0.3.55`. The following
gates pass:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` (10 passed)
- `cargo audit`

The final audit reports no vulnerability and 20 maintenance or soundness
warnings inherited principally from the GTK3/Tauri desktop graph. Keeping this
graph separate prevents desktop-only risk from being confused with the server
backend.

### Frontend

The maintenance batch updates the direct React, TanStack Virtual, Lucide,
Playwright, Testing Library, Vite plugin, ESLint and TypeScript ESLint packages,
plus the root Tauri JavaScript API. Exact transitive overrides close advisories
in `adm-zip`, `postcss`, `flatted`, `brace-expansion`, `dompurify`, `protobufjs`
and `sharp`. Major updates remain protected by the complete frontend
typecheck/lint/test/build gate.
[src: file: frontend/pnpm-workspace.yaml:21-42]

`pnpm audit --audit-level=moderate` now passes and is mandatory in both the
regular CI and aggregate dependency-review workflow. The review deliberately
runs all three audits before failing, so one graph cannot hide findings in
another. [src: file: .github/workflows/ci-test.yml:587-595] [src: file:
.github/workflows/dependency-review.yml:27-63]

After the final direct-dependency refresh, `pnpm audit` reports no known
vulnerabilities and both the root and frontend `pnpm outdated` checks report
that every package is current. No audit allowlist was added.

Validated frontend gates:

- frozen lockfile install
- stable TypeScript 7 native typecheck and production build
- aliased TypeScript 6 API-compatibility typecheck
- ESLint: 0 errors, 92 warnings (budget unchanged)
- Vitest: 230 files, 2,958 tests, including the compiler-alias contract test
- Coverage: 72.09% statements, 67.91% branches, 67.17% functions and
  75.54% lines (all above the CI floors)
- production build
- `pnpm audit --audit-level=moderate`

## TypeScript 7 stable adoption

TypeScript 7.0.2 is the default compiler for `build` and
`typecheck:native`. Because the stable native compiler intentionally ships
without the programmatic API consumed by typescript-eslint, the official
side-by-side arrangement keeps `@typescript/typescript6` aliased as the
`typescript` package and exposes its `tsc6` binary through
`typecheck:legacy`. Both compiler paths are explicit CI gates. [src: file:
frontend/package.json:9-16] [src: file: frontend/package.json:42-64] [src: url:
https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/]

The earlier preview pilot produced zero diagnostics with both compilers on the
same source tree. Its measurements were taken on macOS arm64 with Node 26.4,
pnpm 11 and `/usr/bin/time -l`, three runs per command; they are retained as
historical motivation rather than claimed as stable-release benchmarks:

| Compiler | Command | Median wall time | Median max RSS |
|---|---|---:|---:|
| TypeScript 6.0.3 | `pnpm exec tsc -b --pretty false` | 9.79 s | 963,215,360 B |
| TS7 native preview | `pnpm exec tsgo -b --pretty false` | 2.23 s | 702,955,520 B |

The preview was approximately 77% faster and used 27% less peak memory in that
local sample. Once 7.0.2 became stable, Kronn adopted it for compilation while
retaining TypeScript 6 only for the API-dependent toolchain. A dedicated
manifest regression test prevents either compiler alias or script from being
silently collapsed. [src: file:
frontend/src/__tests__/toolchain-contract.test.ts:1-27]

## Oxlint dual-run pilot

Oxlint runs first as a fast feedback gate, followed by authoritative ESLint in
CI. The generated configuration mirrors implemented rules and documents the
fallback implicitly through the retained ESLint pass. [src: file:
.github/workflows/ci-test.yml:343-352] [src: file:
frontend/.oxlintrc.json:1-18]

Same-machine measurements:

| Linter | Wall time | Max RSS | Diagnostics |
|---|---:|---:|---|
| ESLint 10.3 | 17.26 s | 1,323,696,128 B | 0 errors, 92 warnings |
| Oxlint 1.76 | 1.82 s | 207,142,912 B | 0 errors, 14 warnings |

Oxlint was approximately 89% faster and used 84% less peak memory. It is not a
replacement yet: 78 existing warnings remain observable only through ESLint,
principally React Compiler rules and the project-specific
`no-restricted-syntax` selectors. `no-unsafe-optional-chaining` is disabled in
Oxlint because its current implementation reported five test expressions that
the authoritative ESLint configuration accepts; ESLint continues to enforce
the project policy. [src: file: frontend/.oxlintrc.json:18-83]

## Installer evidence

Local macOS arm64 validation covered:

- building the frozen DOCX/PDF sidecar;
- a warm bundled sidecar smoke test producing both formats;
- a complete Tauri debug application build with the real
  `beforeBuildCommand`, frontend assets, icons and sidecar resource mapping;
- generation of the `.app` during a debug DMG attempt.

The first cold sidecar health/export attempts exceeded short local smoke
timeouts; the subsequent warm smoke passed without changing production timeout
policy. The local DMG script then stopped without an actionable underlying
diagnostic. It must not be presented as installer proof.

Release installer proof therefore remains the existing platform CI matrix:
macOS DMG, Windows NSIS and Linux DEB, followed by the bundled DOCX/PDF smoke
test on each runner. [src: file:
.github/workflows/desktop-build.yml:1-166] The bundle configuration itself
declares the document sidecar resource and desktop installer targets. [src:
file: desktop/src-tauri/tauri.conf.json:35-55]

## Release decision

- Dependency and security maintenance: ready, subject to normal CI.
- TypeScript 7: stable native compiler adopted for builds and typechecks;
  retain the explicit TypeScript 6 compatibility gate for API consumers.
- Oxlint: keep the reversible fast dual-run; ESLint remains authoritative with
  its 92-warning ratchet.
- Installers: require green platform build jobs before release; local macOS
  evidence alone is intentionally insufficient.
