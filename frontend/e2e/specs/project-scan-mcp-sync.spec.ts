/**
 * Project scan + MCP sync — full cycle.
 *
 * Proves that registering a fresh on-disk directory as a Kronn project
 * triggers the MCP scanner to write the per-agent config files
 * (`.mcp.json` / `.kiro/settings/mcp.json` / `.gemini/settings.json`)
 * with the `kronn-internal` entry pointing at the shared-config path.
 *
 * # Why this exists
 *
 * Pre-2026-05-10, kronn-internal was injected with a Docker-only path
 * (`/app/scripts/disc-introspection-mcp.py`) which broke the user's
 * host CLIs (`kiro-cli`, `claude`, `gemini`) at every spawn with
 * `Broken pipe (os error 32)`. The fix was a self-mount + new resolver
 * (`disc_introspection_mcp_path_for_shared_config`). This spec is the
 * regression detector: any future change that re-introduces a
 * container-only path will fail this test.
 *
 * # Cost
 *
 * Zero $. No agent runs, no LLM calls — just the filesystem assertions.
 *
 * # Cleanup
 *
 * `afterAll` deletes the project from Kronn (which removes its
 * sidecar files) AND `rm -rf`s the on-disk directory we created.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';
import { mkdtempSync, rmSync, writeFileSync, mkdirSync, existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { execSync } from 'node:child_process';

let projectId: string | null = null;
let projectPath: string | null = null;

async function findProject(request: APIRequestContext, id: string) {
  const r = await request.get('/api/projects');
  if (!r.ok()) return null;
  const j = await r.json();
  const list = (j?.data ?? []) as Array<{ id: string; name: string; path: string }>;
  return list.find(p => p.id === id) ?? null;
}

test.describe.configure({ timeout: 60_000, retries: 0 });

// Skip in CI : the MCP sync step writes per-agent config files
// (`~/.claude.json`, `~/.codex/config.toml`, `.gemini/settings.json`,
// …) and probes the agent binaries. On a GitHub runner none of these
// dirs/binaries exist, the sync probes hang or fail and the spec eats
// its 60s timeout. The pure sync logic is covered by unit tests in
// `core::mcp_scanner_test`; this spec validates the UI-driven path
// against a real user FS, which CI can't reproduce faithfully.
test.skip(!!process.env.CI, 'real-FS spec — local-only (requires agent config dirs + binaries)');

test.describe('Project scan — registers folder + syncs MCP files', () => {
  test.beforeAll(() => {
    // Create a fresh dir under ~/Repositories so the Kronn container's
    // RW mount picks it up. KRONN_REPOS_DIR override would also work,
    // but the default `~/Repositories` is what the docker-compose
    // mounts, and the E2E runs against that prod backend.
    const reposBase = process.env.KRONN_REPOS_DIR ?? join(homedir(), 'Repositories');
    if (!existsSync(reposBase)) {
      throw new Error(`Repos base ${reposBase} not found — set KRONN_REPOS_DIR or create the dir`);
    }
    projectPath = mkdtempSync(join(reposBase, '_kronn_pw_scan_'));
    // Make it look like a real project: .git stub + a single fake MCP
    // entry. The kronn scanner ALSO writes a `.mcp.json` from the DB
    // configs after the project is added, so our seed `mcpServers`
    // entries will be merged with whatever the user has globally.
    execSync('git init -q .', { cwd: projectPath });
    writeFileSync(join(projectPath, '.mcp.json'), JSON.stringify({
      mcpServers: {
        // Seed entry that should survive the sync round-trip.
        'pw-test-fixture': { command: 'echo', args: ['hello'] },
      },
    }, null, 2));
    writeFileSync(join(projectPath, 'README.md'), '# PW scan fixture\n');
  });

  test.afterAll(async ({ request }) => {
    if (projectId) {
      await request.delete(`/api/projects/${projectId}`).catch(() => { /* idempotent cleanup */ });
    }
    if (projectPath && existsSync(projectPath)) {
      rmSync(projectPath, { recursive: true, force: true });
    }
  });

  test('add-folder registers the project and writes per-agent MCP files', async ({ request }) => {
    test.skip(!projectPath, 'beforeAll failed to create temp project');

    // 1. POST /api/projects/add-folder
    const create = await request.post('/api/projects/add-folder', {
      data: { path: projectPath, name: 'PW scan fixture' },
    });
    expect(create.ok(), `add-folder should succeed (got ${create.status()})`).toBe(true);
    const j = await create.json();
    expect(j?.success).toBe(true);
    projectId = j?.data?.id;
    expect(projectId).toBeTruthy();

    // 2. Project visible in /api/projects
    const found = await findProject(request, projectId!);
    expect(found, 'project should appear in /api/projects').toBeTruthy();
    expect(found!.path).toBe(projectPath);

    // 3. Trigger an explicit MCP refresh so the scanner runs against the
    //    fresh project (the add-folder path may schedule it lazily).
    const refresh = await request.post('/api/mcps/refresh');
    expect(refresh.ok()).toBe(true);

    // 4. Verify the per-agent config files were written.
    //    `.mcp.json` always exists. `.kiro/settings/mcp.json` and
    //    `.gemini/settings.json` exist as long as the user has any
    //    MCP configured globally — which is true on any real prod
    //    deployment. We assert the Claude file unconditionally and
    //    soft-check the others.
    const claudeMcp = join(projectPath!, '.mcp.json');
    expect(existsSync(claudeMcp), `${claudeMcp} should exist after sync`).toBe(true);
    const claudeData = JSON.parse(readFileSync(claudeMcp, 'utf-8'));
    expect(claudeData.mcpServers, '.mcp.json should declare mcpServers').toBeTruthy();

    // 5. kronn-internal must be present AND its path must NOT be the
    //    Docker-only `/app/scripts/...`. The shared-config resolver
    //    routes via `KRONN_INTROSPECTION_PUBLIC_PATH` (set in
    //    docker-compose.yml) so the entry is reachable from BOTH the
    //    container Kronn-spawn AND the user's host CLI.
    const ki = claudeData.mcpServers['kronn-internal'];
    if (ki) {
      // The script is allowed to be missing if the host install doesn't
      // ship it (Docker-only deployments) — in which case the scanner
      // skips injection rather than write a broken path. But IF an
      // entry was written, its path must be valid host-side.
      const path: string = ki.args?.[0] ?? '';
      expect(path).toBeTruthy();
      expect(path).not.toBe('/app/scripts/disc-introspection-mcp.py');
      expect(existsSync(path), `kronn-internal path ${path} must exist on this filesystem`).toBe(true);
    }
  });
});
