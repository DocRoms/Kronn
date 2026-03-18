- **ID**: TD-20260318-drift-detection
- **Area**: Backend + Frontend
- **Severity**: Feature (high-value token optimization)
- **Problem (fact)**: After an AI audit, the `ai/` files become stale as source code evolves. Currently there is no way to detect which sections are outdated. A full re-audit costs ~20K tokens; a targeted re-audit would cost ~2-5K tokens.
- **Impact**: token waste — users re-audit entirely or work with stale context

## Design

### `ai/checksums.json` — generated during audit

```json
{
  "audited_at": "2026-03-18",
  "mappings": [
    {
      "ai_file": "ai/coding-rules.md",
      "audit_step": 4,
      "sources": ["package.json", "Cargo.toml", "tsconfig.json"],
      "checksums": { "package.json": "sha256...", "Cargo.toml": "sha256..." }
    }
  ]
}
```

Each audit step declares which source files it reads. After the audit, the backend hashes those files and stores the mapping.

### Drift check — `GET /api/projects/:id/drift`

On project page load (no LLM call, pure Rust):
1. Read `ai/checksums.json` from the project directory
2. For each mapping, recompute SHA256 of each source file
3. Compare with stored checksums
4. Return: `{ stale_sections: ["ai/coding-rules.md", "ai/architecture/overview.md"], details: [...] }`

### UI — project card

- Show audit date: `[✓ Audited — 14 mars]`
- If stale sections: `[⚠ 2 sections obsolètes]`
- Button: "Mettre à jour (2 sections, ~3K tokens)"
- After update: badge refreshes, date updates, system message shows tokens consumed

### Selective re-audit — `POST /api/projects/:id/partial-audit`

Accepts `{ steps: [4, 8] }`. Runs only the specified audit steps with the same prompts. Updates `ai/checksums.json` with new hashes. Much cheaper than a full 10-step audit.

### Step-to-source mapping (to implement)

Each ANALYSIS_STEP in `projects.rs` needs a `sources` field declaring which files it reads:

| Step | Target ai/ file | Source files to hash |
|------|----------------|---------------------|
| 1 | ai/index.md | README.md, package.json, Cargo.toml, docker-compose.yml, Makefile |
| 2 | ai/glossary.md | (derived from other steps, no direct sources) |
| 3 | ai/repo-map.md | directory structure (hash of `find . -type f` output) |
| 4 | ai/coding-rules.md | package.json, Cargo.toml, tsconfig.json, .eslintrc*, rustfmt.toml |
| 5 | ai/testing-quality.md | package.json, Cargo.toml, .github/workflows/*, vitest.config.*, jest.config.* |
| 6 | ai/operations/debug-operations.md | docker-compose.yml, Makefile, Dockerfile* |
| 7 | ai/operations/mcp-servers.md | .mcp.json |
| 8 | ai/architecture/overview.md | docker-compose.yml, main entrypoints (src/main.*, src/lib.*, src/App.*) |
| 9 | ai/inconsistencies-tech-debt.md | (full scan — hash all source files, or use git diff since audit date) |
| 10 | REVIEW | (re-read all ai/ files — no external sources) |

## Estimated effort

- Backend: checksums generation during audit (~2h)
- Backend: drift check endpoint (~1h)
- Backend: selective re-audit (~3-4h)
- Frontend: drift badge + update button (~2h)
- Tests (~2h)
- **Total: ~10-12h**

## Token economics

- Drift check: 0 tokens (pure computation)
- Full re-audit: ~20K tokens
- Targeted re-audit (2 stale sections): ~3-5K tokens
- **Savings per re-audit: ~15-17K tokens (75-85%)**
- Annual savings (monthly re-audit): ~180-200K tokens
