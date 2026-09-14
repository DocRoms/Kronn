import { test, expect } from '../fixtures/kronn-fixture';
import { SettingsPage } from '../pages/SettingsPage';

for (const width of [1440, 390]) {
  for (const [agent, key, runtime] of [['ClaudeCode', 'claude_code', 'agent:claude-code'], ['Codex', 'codex', 'agent:codex']]) {
    test(`${agent} effort persists and clears safely at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      const at = '2026-09-14T00:00:00Z';
      const view = { runtime_target_id: runtime, agent_type: agent, live_refresh_ok: true, stale: false,
        models: ['deep', 'small'].map(id => ({ id: `${runtime}:${id}`, runtime_target_id: runtime,
          agent_type: agent, model_id: id, display_name: id, provenance: 'live', availability: 'available',
          capabilities: ['chat'], reasoning_modes: id === 'deep' ? ['low', 'high'] : ['low'],
          manual_origin: false, first_seen_at: at, last_seen_at: at, last_checked_at: at,
          created_at: at, updated_at: at,
        })),
      };
      const empty = () => ({ economy: null, default: null, reasoning: null });
      let tiers: Record<string, Record<string, string | null>> = Object.fromEntries(
        ['claude_code', 'codex', 'open_code', 'gemini_cli', 'kiro', 'vibe', 'copilot_cli', 'ollama', 'lite_llm', 'nvidia'].map(k => [k, empty()]),
      );
      tiers[key] = { ...empty(), default: 'deep' };
      tiers.ollama.default = 'operator-local-model';
      const writes: typeof tiers[] = [];
      await page.route('**/api/agents', route => route.fulfill({ json: { success: true, data: [{
        name: agent, agent_type: agent, installed: true, enabled: true, path: null, version: null,
        latest_version: null, origin: 'host', install_command: '', host_managed: false, host_label: null,
        runtime_available: true, auth_ready: true, rtk_available: false, rtk_hook_configured: false,
      }], error: null } }));
      await page.route('**/api/model-catalogs', route => route.fulfill({ json: { success: true, data: { targets: [view] }, error: null } }));
      await page.route('**/api/model-catalogs/refresh', route => route.fulfill({ json: { success: true, data: view, error: null } }));
      await page.route('**/api/config/model-tiers', route => {
        if (route.request().method() === 'POST') {
          tiers = route.request().postDataJSON();
          writes.push(structuredClone(tiers));
        }
        return route.fulfill({ json: { success: true, data: tiers, error: null } });
      });
      const settings = new SettingsPage(page);
      const open = async () => {
        await page.goto('/');
        await page.locator('[data-tour-id="nav-settings"]').click();
        await settings.openAgentConfiguration(agent);
      };
      await open();
      const effort = settings.effortPreset(agent, 'default');
      await expect(effort).toHaveValue('');
      await expect(effort).toBeEnabled();
      await effort.selectOption('high');
      await expect.poll(() => writes.length).toBe(1);
      expect(tiers[key]).toMatchObject({ default: 'deep', default_effort: 'high' });
      await open();
      await expect(effort).toHaveValue('high');
      const picker = settings.modelPreset(agent, 'default');
      await picker.focus();
      await page.getByRole('option', { name: 'small', exact: true }).click();
      await expect.poll(() => writes.length).toBe(2);
      expect(tiers[key]).toMatchObject({ default: 'small', default_effort: null });
      await expect(effort).toHaveValue('');
      tiers[key].default_effort = 'retired-mode';
      await open();
      await expect(effort).toHaveValue('retired-mode');
      await expect(effort).toBeEnabled();
      await effort.selectOption('');
      await expect.poll(() => writes.length).toBe(3);
      expect(tiers[key]).toMatchObject({ default: 'small', default_effort: null });
      for (const saved of writes) expect(saved.ollama.default).toBe('operator-local-model');
      await effort.scrollIntoViewIfNeeded();
      const box = await effort.boundingBox();
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(0);
      expect(box!.x + box!.width).toBeLessThanOrEqual(width + 1);
    });
  }
}
