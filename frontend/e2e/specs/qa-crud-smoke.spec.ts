/**
 * Quick API (QA) — CRUD smoke + run-validation smoke.
 *
 * Real-run coverage of the QA execution path lives in the linear-
 * workflow spec (an `ApiCall` step exercises the same backend
 * executor under a different surface). This spec is the lighter
 * gate: it pins the public CRUD contract (create / list / get /
 * delete) AND the variable-validation branch of `POST /api/quick-apis/:id/run`
 * (a missing required variable must surface a `success: false`
 * envelope, NOT a 500).
 *
 * Why we don't drive a real HTTP request from here: every saved QA
 * is bound to an `api_config_id` (Chartbeat / Adobe / SpeedCurve /
 * future Custom plugin), each with their own auth + rate limits.
 * Running for real on this spec would either burn the user's
 * provider creds or require a public no-auth plugin — better
 * deferred to the explicit linear-workflow spec.
 *
 * # Cost
 *
 * Zero $. No agent runs, no external HTTP calls.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

interface ApiPluginConfig { config_id: string; label: string; plugin_slug: string }

async function pickAnyApiPlugin(request: APIRequestContext): Promise<ApiPluginConfig | null> {
  const r = await request.get('/api/mcps');
  if (!r.ok()) return null;
  const j = await r.json();
  const servers: Array<{ id: string; name: string; api_spec: unknown }> = j?.data?.servers ?? [];
  const configs: Array<{ id: string; label: string; server_id: string }> = j?.data?.configs ?? [];
  for (const cfg of configs) {
    const srv = servers.find(s => s.id === cfg.server_id);
    if (!srv?.api_spec) continue;
    return {
      config_id: cfg.id,
      label: cfg.label,
      // The plugin slug is the server name lowercased on the QP side
      // (cf. `quickApisApi.create` payload). We mirror that mapping.
      plugin_slug: srv.name.toLowerCase().replace(/\s+/g, '-'),
    };
  }
  return null;
}

let qaId: string | null = null;

test.describe.configure({ timeout: 30_000, retries: 0 });

test.describe('Quick APIs — CRUD smoke + run validation', () => {
  test.afterAll(async ({ request }) => {
    if (qaId) {
      await request.delete(`/api/quick-apis/${qaId}`).catch(() => { /* idempotent cleanup */ });
    }
  });

  test('CRUD round-trip works against the live backend', async ({ request }) => {
    const plugin = await pickAnyApiPlugin(request);
    test.skip(!plugin, 'No API plugin configured on this backend — nothing to bind a QA to');

    // 1. Create.
    const created = await request.post('/api/quick-apis', {
      data: {
        name: `PW QA smoke ${Date.now()}`,
        icon: '🧪',
        description: 'PW CRUD smoke fixture',
        project_id: null,
        api_plugin_slug: plugin!.plugin_slug,
        api_config_id: plugin!.config_id,
        api_endpoint_path: '/_kronn_pw_smoke_',
        api_method: 'GET',
        api_query: { foo: '{{ticket}}' },
        variables: [{ name: 'ticket', label: 'Ticket', placeholder: 'EW-1', required: true }],
      },
    });
    expect(created.ok(), `create returned ${created.status()}`).toBe(true);
    const cj = await created.json();
    expect(cj?.success).toBe(true);
    qaId = cj?.data?.id ?? null;
    expect(qaId).toBeTruthy();

    // 2. List → find our id.
    const list = await request.get('/api/quick-apis');
    const lj = await list.json();
    const found = (lj?.data ?? []).find((q: { id: string }) => q.id === qaId);
    expect(found, 'created QA must surface in /api/quick-apis').toBeTruthy();
    expect(found.api_plugin_slug).toBe(plugin!.plugin_slug);

    // 3. Run validation: missing required `ticket` should bail with
    //    a structured error, not a 500.
    const runResp = await request.post(`/api/quick-apis/${qaId}/run`, {
      data: { variables: {} },
    });
    expect(runResp.ok(), 'run endpoint always returns 200 with envelope').toBe(true);
    const rj = await runResp.json();
    expect(rj?.success, 'missing required var must envelope-error, not 500').toBe(false);
    expect(String(rj?.error ?? '')).toMatch(/Variable obligatoire manquante|ticket/i);

    // 4. Delete.
    const del = await request.delete(`/api/quick-apis/${qaId}`);
    expect(del.ok()).toBe(true);
    qaId = null;
  });
});
