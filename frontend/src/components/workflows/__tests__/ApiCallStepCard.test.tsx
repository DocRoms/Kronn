// Unit tests for ApiCallStepCard — the wizard panel for StepType::ApiCall.
//
// Scope: empty-state when no API plugin is configured, plugin+endpoint
// selection cascades into the step, query-param rows add/remove, Test
// button forwards to /test-api-call and populates the response viewer,
// clicking a JSON leaf auto-fills the extract path, and live preview
// through /test-extract.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { ApiSpec, McpServer, McpConfigDisplay, WorkflowStep } from '../../../types/generated';

const { testApiCallMock, testExtractMock } = vi.hoisted(() => ({
  testApiCallMock: vi.fn(),
  testExtractMock: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  workflows: {
    testApiCall: testApiCallMock as never,
    testExtract: testExtractMock as never,
  },
}));

import { ApiCallStepCard, previewString, type ApiPluginOption } from '../ApiCallStepCard';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const mkServer = (id: string, name: string, apiSpec: ApiSpec | null): McpServer => ({
  id,
  name,
  description: '',
  transport: 'ApiOnly',
  source: 'Registry',
  api_spec: apiSpec,
});

const mkConfig = (id: string, server_id: string): McpConfigDisplay => ({
  id,
  server_id,
  server_name: 'Chartbeat',
  label: 'Prod',
  env_keys: [],
  env_masked: [],
  args_override: null,
  is_global: false,
  include_general: false,
  config_hash: 'abc',
  project_ids: ['proj-1'],
  project_names: [],
  secrets_broken: false,
  host_sync: 'None',
});

const chartbeatServer = mkServer('chartbeat', 'Chartbeat', {
  base_url: 'https://api.chartbeat.com',
  auth: { ApiKeyQuery: { param_name: 'apikey', env_key: 'CHARTBEAT_KEY' } },
  endpoints: [
    { path: '/live/toppages/v4', method: 'GET', description: 'Top live pages' },
    { path: '/live/summary/v4', method: 'GET', description: 'Summary' },
  ],
  docs_url: null,
  config_keys: [],
});

const mkStep = (over: Partial<WorkflowStep> = {}): WorkflowStep => ({
  name: 'fetch',
  step_type: { type: 'ApiCall' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'Structured' },
  mcp_config_ids: [],
  agent_settings: null,
  on_result: [],
  stall_timeout_secs: null,
  retry: null,
  delay_after_secs: null,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  batch_quick_prompt_id: null,
  batch_items_from: null,
  batch_wait_for_completion: null,
  batch_max_items: null,
  batch_workspace_mode: null,
  batch_chain_prompt_ids: [],
  notify_config: null,
  api_plugin_slug: null,
  api_config_id: null,
  api_endpoint_path: null,
  api_method: null,
  api_query: null,
  api_headers: null,
  api_body: null,
  api_extract: null,
  api_pagination: null,
  api_timeout_ms: null,
  api_max_retries: null,
  api_output_var: null,
  ...over,
});

describe('previewString — one-line preview chip', () => {
  it('shows array contents (truncated) instead of "Array(N)" for the wildcard case', () => {
    // The Chartbeat scenario: extract `$.toppages[*].path` resolves to
    // an array of 5 strings. The preview must render the first 3 items
    // with `… (+N)` suffix — not just "Array(5)".
    const arr = [
      'fr.euronews.com/',
      'fr.euronews.com/voyages/2026/04/25/voyage-espagne',
      'fr.euronews.com/my-europe/2026/04/24/quels-sont',
      'fr.euronews.com/sante/2026/04/25/biere',
      'fr.euronews.com/live',
    ];
    const out = previewString(arr);
    expect(out).toContain('"fr.euronews.com/"');
    expect(out).toContain('… (+2)');
    expect(out.startsWith('[')).toBe(true);
    expect(out.endsWith(']')).toBe(true);
  });

  it('shows top-level keys for an object instead of "Object"', () => {
    const obj = { id: 42, title: 'Hello', meta: { nested: true } };
    const out = previewString(obj);
    expect(out).toContain('id: 42');
    expect(out).toContain('title: "Hello"');
    // Nested objects collapse to `{…}` to keep the chip on one line.
    expect(out).toContain('meta: {…}');
  });

  it('caps depth at 1 (deep arrays render as `[N]`, deep objects as `{…}`)', () => {
    expect(previewString({ x: [1, 2, 3] })).toBe('{x: [3]}');
    expect(previewString([{ a: 1 }, { a: 2 }])).toBe('[{…}, {…}]');
  });

  it('handles edge cases (null, empty array, empty object, scalar)', () => {
    expect(previewString(null)).toBe('null');
    expect(previewString([])).toBe('[]');
    expect(previewString({})).toBe('{}');
    expect(previewString(42)).toBe('42');
    expect(previewString(true)).toBe('true');
  });
});

describe('ApiCallStepCard', () => {
  beforeEach(() => {
    testApiCallMock.mockReset();
    testExtractMock.mockReset();
  });

  // ─── Empty state ────────────────────────────────────────────────

  it('renders an empty-state message when no API plugin is configured', () => {
    render(
      <ApiCallStepCard
        step={mkStep()}
        onChange={() => {}}
        availableApiPlugins={[]}
        projectId="proj-1"
        t={t}
      />,
    );
    expect(screen.getByText('wf.apicall.notSupported')).toBeInTheDocument();
    // No plugin picker rendered in empty state.
    expect(screen.queryByText('wf.apicall.pluginPicker')).not.toBeInTheDocument();
  });

  // ─── Plugin + endpoint pickers ──────────────────────────────────

  // ─── AI helper opt-in (0.5.2) ───────────────────────────────────
  // The helper bubble is rendered inside the card header only when at
  // least one agent is installed locally. Without the prop the trigger
  // button must stay hidden — running Kronn on a host with zero agents
  // shouldn't surface a button that can't do anything.

  it('hides the AI helper trigger when no agent is installed', () => {
    const plugins: ApiPluginOption[] = [
      { server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') },
    ];
    render(
      <ApiCallStepCard
        step={mkStep()}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        installedAgents={[]}
        t={t}
      />,
    );
    expect(screen.queryByText('wf.apicall.helper.trigger')).not.toBeInTheDocument();
  });

  // ─── Click-to-pick on JSON tree (DWIM wildcard for keys in arrays) ──

  it('clicking a key INSIDE an array item generates a wildcard path (Tous les <field>)', async () => {
    // Real Chartbeat-shape response: clicking the `path` key under
    // `toppages[0]` must give `$.toppages[*].path` (iterate over ALL
    // items), not `$.toppages[0].path` (just the first one). This is the
    // single change that makes click-to-pick "Just Work" for fan-out.
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 12,
      envelope: {
        data: {
          toppages: [
            { path: 'fr.euronews.com/', visitors: 32 },
            { path: 'fr.euronews.com/voyages/', visitors: 28 },
          ],
        },
        status: 'OK',
        summary: '2 items',
      },
      error: null,
    });
    const onChange = vi.fn();
    const plugins: ApiPluginOption[] = [
      { server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') },
    ];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_endpoint_path: '/live/toppages/v4' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testApiCallMock).toHaveBeenCalled());
    // The `path` key is repeated at every array index (`[0].path`,
    // `[1].path`, …). All of them share the SAME wildcard target — the
    // critical assertion is "whichever the user clicks, they get
    // `$.toppages[*].path`", which is the whole point of this fix.
    const pathKeyBtns = await screen.findAllByRole('button', { name: /^path$/ });
    expect(pathKeyBtns.length).toBeGreaterThan(0);
    fireEvent.click(pathKeyBtns[0]);
    const calls = onChange.mock.calls.map(c => c[0]);
    const extractCall = calls.find(c => c.api_extract);
    expect(extractCall?.api_extract?.path).toBe('$.toppages[*].path');
  });

  it('clicking a leaf VALUE inside an array item keeps the specific [N] index', async () => {
    // Counterpart to the wildcard rule: when the user clicks the actual
    // string `"fr.euronews.com/"` (a leaf), they want THAT specific
    // value, not "all paths". The path stays indexed.
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 12,
      envelope: {
        data: {
          toppages: [
            { path: 'fr.euronews.com/', visitors: 32 },
            { path: 'fr.euronews.com/voyages/', visitors: 28 },
          ],
        },
        status: 'OK',
        summary: '2 items',
      },
      error: null,
    });
    const onChange = vi.fn();
    const plugins: ApiPluginOption[] = [
      { server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') },
    ];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_endpoint_path: '/live/toppages/v4' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testApiCallMock).toHaveBeenCalled());
    // The first leaf rendering of `"fr.euronews.com/"` lives inside the
    // JSON tree (the second match is the suggest-chip sample). We pick
    // the one inside the response viewer, which is the first occurrence
    // — both share the literal text but only the JSON one is a button.
    const leafBtns = await screen.findAllByRole('button', { name: /"fr\.euronews\.com\/"/ });
    fireEvent.click(leafBtns[0]);
    const calls = onChange.mock.calls.map(c => c[0]);
    const extractCall = calls.find(c => c.api_extract);
    expect(extractCall?.api_extract?.path).toBe('$.toppages[0].path');
  });

  it('renders an Auth-managed read-only row for ApiKeyQuery plugins', () => {
    // Chartbeat → `ApiKeyQuery { param_name: 'apikey' }`. The card must
    // surface the slot as read-only so the user knows their API key is
    // already wired (and so they don't paste it again into the query rows).
    const plugins: ApiPluginOption[] = [
      { server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') },
    ];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_config_id: 'cfg-1' })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    expect(screen.getByText('wf.apicall.authManagedTitle')).toBeInTheDocument();
    // The query-param key (`apikey`) is rendered as the slot label.
    expect(screen.getByText('apikey')).toBeInTheDocument();
    // The value stays masked until the user clicks the eye.
    expect(screen.getByText('••••••••')).toBeInTheDocument();
  });

  it('shows the AI helper trigger when at least one agent is installed', () => {
    const plugins: ApiPluginOption[] = [
      { server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') },
    ];
    render(
      <ApiCallStepCard
        step={mkStep()}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        installedAgents={['ClaudeCode']}
        t={t}
      />,
    );
    expect(screen.getByText('wf.apicall.helper.trigger')).toBeInTheDocument();
  });

  it('lists every configured API plugin in the picker', () => {
    const plugins: ApiPluginOption[] = [
      { server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') },
    ];
    render(
      <ApiCallStepCard
        step={mkStep()}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // The option text is `<server.name> — <config.label>`.
    expect(screen.getByText('Chartbeat — Prod')).toBeInTheDocument();
  });

  it('selecting a plugin propagates slug + config_id and resets endpoint', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'stale', api_endpoint_path: '/stale' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    const pluginSelect = screen.getAllByRole('combobox')[0];
    // The picker is now bound on `config.id` (so two configs of the
    // same plugin are distinguishable), but it still writes BOTH the
    // slug and the config_id on change.
    fireEvent.change(pluginSelect, { target: { value: 'cfg-1' } });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        api_plugin_slug: 'chartbeat',
        api_config_id: 'cfg-1',
        // Endpoint path must be reset — the new plugin may not expose '/stale'.
        api_endpoint_path: null,
        api_method: null,
      }),
    );
  });

  it('renders the Headers editor inline (NOT behind the Advanced toggle) so AI-applied headers are immediately visible', () => {
    // Regression for: "I clicked Apply on a User-Agent suggestion and
    // nothing changed". Root cause was the Headers editor being inside
    // the Advanced collapsible — the apply DID land in `step.api_headers`
    // but the user couldn't see it without expanding the section. Headers
    // are too common in real-world API plugins (User-Agent for GitHub,
    // X-API-Version for Adobe, Accept for custom mime types) to hide.
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_config_id: 'cfg-1',
          api_headers: { 'User-Agent': 'Kronn-Workflow/1.0' },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // Headers editor legend visible immediately — even though the
    // Advanced toggle is closed (no body / method / timeout set).
    expect(screen.getByText('wf.apicall.headers')).toBeInTheDocument();
    // The applied header row is rendered.
    expect(screen.getByText('User-Agent')).toBeInTheDocument();
    // The Advanced toggle stays closed when no advanced field is set.
    const toggle = screen.getByRole('button', { name: /wf\.apicall\.advancedToggle/ });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });

  it('auto-expands Advanced when a body/method/timeout field is set on the step', () => {
    // The auto-expand behaviour still applies for fields that are
    // genuinely advanced (rare: body, method override, custom timeout).
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_config_id: 'cfg-1',
          api_method: 'POST',
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    const toggle = screen.getByRole('button', { name: /wf\.apicall\.advancedToggle/ });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
  });

  it('detects {owner}/{repo} placeholders in the endpoint and renders one input per token', () => {
    // GitHub-shape regression: when the user picks `/repos/{owner}/{repo}`,
    // dedicated inputs surface so they don't have to manually splice the
    // values into the path string.
    const githubServer: McpServer = {
      ...chartbeatServer, id: 'mcp-github', name: 'GitHub',
      api_spec: { ...chartbeatServer.api_spec!, base_url: 'https://api.github.com' },
    };
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'mcp-github',
          api_config_id: 'cfg-1',
          api_endpoint_path: '/repos/{owner}/{repo}/issues',
        })}
        onChange={onChange}
        availableApiPlugins={[
          { server: githubServer, config: mkConfig('cfg-1', 'mcp-github') },
        ]}
        projectId="proj-1"
        t={t}
      />,
    );
    // Both placeholders are rendered as labels — `{owner}` and `{repo}`.
    expect(screen.getByText('{owner}')).toBeInTheDocument();
    expect(screen.getByText('{repo}')).toBeInTheDocument();
    // The path-params editor's title is shown.
    expect(screen.getByText(/wf\.apicall\.pathParamsTitle/)).toBeInTheDocument();
  });

  it('typing into a path-param input writes to step.api_path_params and resolves the preview', () => {
    const githubServer: McpServer = {
      ...chartbeatServer, id: 'mcp-github', name: 'GitHub',
      api_spec: { ...chartbeatServer.api_spec!, base_url: 'https://api.github.com' },
    };
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'mcp-github',
          api_config_id: 'cfg-1',
          api_endpoint_path: '/repos/{owner}/{repo}',
        })}
        onChange={onChange}
        availableApiPlugins={[
          { server: githubServer, config: mkConfig('cfg-1', 'mcp-github') },
        ]}
        projectId="proj-1"
        t={t}
      />,
    );
    // Locate the {owner} input via its placeholder which is i18n-driven.
    const ownerInput = screen.getByPlaceholderText(/wf\.apicall\.pathParamsPlaceholder.*owner/);
    fireEvent.change(ownerInput, { target: { value: 'anthropics' } });
    expect(onChange).toHaveBeenCalledWith({
      api_path_params: { owner: 'anthropics' },
    });
  });

  it('hides the path-params editor when the endpoint has no {tokens}', () => {
    const githubServer: McpServer = {
      ...chartbeatServer, id: 'mcp-github', name: 'GitHub',
      api_spec: { ...chartbeatServer.api_spec!, base_url: 'https://api.github.com' },
    };
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'mcp-github',
          api_config_id: 'cfg-1',
          api_endpoint_path: '/user',
        })}
        onChange={() => {}}
        availableApiPlugins={[
          { server: githubServer, config: mkConfig('cfg-1', 'mcp-github') },
        ]}
        projectId="proj-1"
        t={t}
      />,
    );
    expect(screen.queryByText(/wf\.apicall\.pathParamsTitle/)).not.toBeInTheDocument();
  });

  it('two configs of the same plugin are distinguishable in the picker (perso vs org)', () => {
    // The killer regression: a user with both a personal GitHub PAT and
    // an org-bound (Euronews) PAT pointing at the SAME plugin (`mcp-github`)
    // must be able to pick which token Kronn uses. The picker keys on
    // `config.id` rather than `server.id`, otherwise both options
    // collapse onto the same value and the wizard always picks the
    // first config silently.
    const githubServer: McpServer = {
      ...chartbeatServer, id: 'mcp-github', name: 'GitHub',
      api_spec: { ...chartbeatServer.api_spec!, base_url: 'https://api.github.com' },
    };
    const persoConfig = { ...mkConfig('cfg-perso', 'mcp-github'), label: 'Perso' };
    const orgConfig = { ...mkConfig('cfg-org', 'mcp-github'), label: 'Euronews' };
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep()}
        onChange={onChange}
        availableApiPlugins={[
          { server: githubServer, config: persoConfig },
          { server: githubServer, config: orgConfig },
        ]}
        projectId="proj-1"
        t={t}
      />,
    );
    // Both options appear with the config label visible.
    expect(screen.getByText('GitHub — Perso')).toBeInTheDocument();
    expect(screen.getByText('GitHub — Euronews')).toBeInTheDocument();

    // Picking the org one writes its config_id verbatim.
    const pluginSelect = screen.getAllByRole('combobox')[0];
    fireEvent.change(pluginSelect, { target: { value: 'cfg-org' } });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        api_plugin_slug: 'mcp-github',
        api_config_id: 'cfg-org',
      }),
    );
  });

  it('endpoint picker exposes every spec endpoint as a datalist option (path = value, method+desc visible)', () => {
    // The wizard switched from `<select>` to `<input + datalist>` so that
    // plugins with path placeholders (`{owner}/{repo}`, GitHub-style) can
    // be edited inline after picking. Each spec endpoint must still
    // surface as a datalist option whose `value` is the path — that's
    // what populates the input on click.
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const { container } = render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    const datalistOptions = container.querySelectorAll('datalist option');
    const paths = Array.from(datalistOptions).map(o => (o as HTMLOptionElement).value);
    expect(paths).toContain('/live/toppages/v4');
    expect(paths).toContain('/live/summary/v4');
  });

  // ─── Test button ────────────────────────────────────────────────

  it('Test button is disabled until plugin + endpoint are selected', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // Missing endpoint — Test stays disabled.
    expect(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ })).toBeDisabled();
  });

  it('clicking Test forwards step + projectId and fills the response viewer on success', async () => {
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 42,
      envelope: { data: { pages: [{ title: 'Hello' }] }, status: 'OK', summary: 'got 1' },
      error: null,
    });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_endpoint_path: '/live/toppages/v4' })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-42"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testApiCallMock).toHaveBeenCalledTimes(1));
    const call = testApiCallMock.mock.calls[0][0];
    expect(call.project_id).toBe('proj-42');
    expect(call.step.api_plugin_slug).toBe('chartbeat');
    // The response viewer renders once the response is set. The sample
    // value `"Hello"` shows up both inside the JsonTreeViewer and as the
    // preview of the auto-derived suggestion chip — using `getAllByText`
    // tolerates that multiplicity (the bug was: it was failing because
    // it expected a single match).
    await waitFor(() => expect(screen.getAllByText(/"Hello"/).length).toBeGreaterThan(0));
  });

  it('Test the call strips api_extract from the request so the viewer always shows the FULL JSON', async () => {
    // Regression: if the step already has an extract path set, the
    // backend used to apply it and return the extracted scalar/array
    // — meaning re-testing later showed only that, not the full body.
    // The wizard now sends a copy of the step with `api_extract: null`
    // so the viewer keeps the raw response and the live preview (right
    // panel) handles extraction client-side via /test-extract.
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 5,
      envelope: { data: { pages: [{ title: 'A' }] }, status: 'OK', summary: 'x' },
      error: null,
    });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_config_id: 'cfg-1',
          api_endpoint_path: '/live/toppages/v4',
          // The user previously picked an extract path. The next test
          // call must NOT honour it server-side.
          api_extract: { path: '$.pages[*].title', fallback: null, fail_on_empty: false },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testApiCallMock).toHaveBeenCalled());
    const sentStep = testApiCallMock.mock.calls[0][0].step;
    expect(sentStep.api_extract).toBeNull();
  });

  it('Test failure surfaces the error inline without clearing the previous response', async () => {
    testApiCallMock.mockResolvedValue({
      success: false,
      duration_ms: 0,
      envelope: null,
      error: 'HTTP 403 — Forbidden',
    });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_endpoint_path: '/live/toppages/v4' })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(/403/));
  });

  // ─── Click-to-pick ──────────────────────────────────────────────

  it('clicking a JSON leaf node fills the extract path with the JSONPath', async () => {
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 10,
      envelope: { data: { total: 42, issues: [{ key: 'KR-1' }] }, status: 'OK', summary: '' },
      error: null,
    });
    const onChange = vi.fn();
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_endpoint_path: '/live/toppages/v4' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    // Wait for the response to render.
    const leaf = await screen.findByRole('button', { name: '42' });
    onChange.mockClear();
    fireEvent.click(leaf);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        api_extract: expect.objectContaining({ path: '$.total' }),
      }),
    );
  });

  it('clicking an array node picks the wildcard path $.field[*]', async () => {
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 10,
      envelope: { data: { issues: [{ key: 'KR-1' }, { key: 'KR-2' }] }, status: 'OK', summary: '' },
      error: null,
    });
    const onChange = vi.fn();
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_endpoint_path: '/live/toppages/v4' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    // Array markers render as "[2]" (count). Clicking gives wildcard path.
    const arrayMarker = await screen.findByRole('button', { name: '[2]' });
    onChange.mockClear();
    fireEvent.click(arrayMarker);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        api_extract: expect.objectContaining({ path: '$.issues[*]' }),
      }),
    );
  });

  // ─── Query params editor ────────────────────────────────────────

  it('adding a query param row propagates a new api_query map', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // Query editor's value placeholder is unique; navigate to the
    // sibling key input. (Headers editor uses "application/json" as
    // its value placeholder, so the two editors no longer collide.)
    const valueInput = screen.getByPlaceholderText('value (can use {{steps.X.data}})');
    const keyInput = valueInput.previousElementSibling as HTMLInputElement;
    fireEvent.change(keyInput, { target: { value: 'host' } });
    fireEvent.change(valueInput, { target: { value: 'euronews.com' } });
    // Two `+` buttons exist (query + headers). Pick the one inside the
    // query row.
    const addBtn = valueInput.nextElementSibling as HTMLButtonElement;
    fireEvent.click(addBtn);
    expect(onChange).toHaveBeenCalledWith({ api_query: { host: 'euronews.com' } });
  });

  it('auto-commits a draft query row when focus leaves the editor (regression: 400 Chartbeat host param)', () => {
    // Regression: a real user typed `host = www.euronews.com` in the
    // draft inputs without clicking the `+`, then hit Test → the call
    // went out without `host`, Chartbeat returned 400. The fix is
    // commit-on-blur: when focus leaves the row entirely (relatedTarget
    // outside) and the key is non-empty, we add the row implicitly.
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <div>
        <ApiCallStepCard
          step={mkStep({ api_plugin_slug: 'chartbeat' })}
          onChange={onChange}
          availableApiPlugins={plugins}
          projectId="proj-1"
          t={t}
        />
        <button data-testid="outside-focus-target">outside</button>
      </div>,
    );
    const valueInput = screen.getByPlaceholderText('value (can use {{steps.X.data}})');
    const keyInput = valueInput.previousElementSibling as HTMLInputElement;
    fireEvent.change(keyInput, { target: { value: 'host' } });
    fireEvent.change(valueInput, { target: { value: 'www.euronews.com' } });
    onChange.mockClear();
    // User clicks Test (or anywhere outside the row) without pressing +.
    // The blur from valueInput fires with relatedTarget = outside button,
    // outside the row → commitOnBlur fires, addRow is called.
    const outside = screen.getByTestId('outside-focus-target');
    fireEvent.blur(valueInput, { relatedTarget: outside });
    expect(onChange).toHaveBeenCalledWith({ api_query: { host: 'www.euronews.com' } });
  });

  it('blur within the same row (key→value) does NOT auto-commit (lets user finish typing)', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    const valueInput = screen.getByPlaceholderText('value (can use {{steps.X.data}})');
    const keyInput = valueInput.previousElementSibling as HTMLInputElement;
    fireEvent.change(keyInput, { target: { value: 'host' } });
    onChange.mockClear();
    // Tab from key to value → blur on key with relatedTarget = value (still
    // in the row). Must NOT commit; user is still typing.
    fireEvent.blur(keyInput, { relatedTarget: valueInput });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('removing the last query param sets api_query back to null (not {})', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_query: { host: 'ex.com' } })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByLabelText('Remove host'));
    // Null (not empty map) keeps the backend deserializer's contract.
    expect(onChange).toHaveBeenCalledWith({ api_query: null });
  });

  // ─── Live preview via /test-extract ──────────────────────────────

  it('editing the extract path triggers a debounced /test-extract call', async () => {
    testApiCallMock.mockResolvedValue({
      success: true,
      duration_ms: 10,
      envelope: { data: { total: 42 }, status: 'OK', summary: '' },
      error: null,
    });
    testExtractMock.mockResolvedValue({
      value: 42,
      value_type: 'number',
      is_empty: false,
      error: null,
    });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_endpoint_path: '/live/toppages/v4',
          api_extract: { path: '$.total', fallback: null, fail_on_empty: false },
        })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // Run a test so `sample` is populated — the preview only fires when
    // both sample and path are set.
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testApiCallMock).toHaveBeenCalled());
    await waitFor(() => expect(testExtractMock).toHaveBeenCalled(), { timeout: 1000 });
    expect(testExtractMock.mock.calls[0][0]).toMatchObject({
      path: '$.total',
    });
  });

  // ─── NextStepBanner (P1.3 — batch wiring validation) ─────────────

  it('banner is hidden when there is no next step', async () => {
    testApiCallMock.mockResolvedValue({
      success: true, duration_ms: 5,
      envelope: { data: { total: 42 }, status: 'OK', summary: '' }, error: null,
    });
    testExtractMock.mockResolvedValue({ value: 42, value_type: 'number', is_empty: false, error: null });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_endpoint_path: '/live/toppages/v4',
          api_extract: { path: '$.total', fallback: null, fail_on_empty: false },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testExtractMock).toHaveBeenCalled());
    // No next step — no banner shown regardless of data type.
    expect(screen.queryByText('wf.apicall.nextStepBatchOk')).not.toBeInTheDocument();
    expect(screen.queryByText(/wf\.apicall\.nextStepBatchMismatch/)).not.toBeInTheDocument();
  });

  it('banner is hidden when next step is Agent (accepts any shape)', async () => {
    testApiCallMock.mockResolvedValue({
      success: true, duration_ms: 5,
      envelope: { data: 'a string', status: 'OK', summary: '' }, error: null,
    });
    testExtractMock.mockResolvedValue({ value: 'a string', value_type: 'string', is_empty: false, error: null });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_endpoint_path: '/live/toppages/v4',
          api_extract: { path: '$.title', fallback: null, fail_on_empty: false },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        nextStepType={{ type: 'Agent' }}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testExtractMock).toHaveBeenCalled());
    // Agent next step = scalar is fine, no wiring concern.
    expect(screen.queryByText(/nextStepBatch/)).not.toBeInTheDocument();
  });

  it('banner shows ✓ when next step is BatchQuickPrompt and data is array', async () => {
    testApiCallMock.mockResolvedValue({
      success: true, duration_ms: 5,
      envelope: { data: ['a', 'b', 'c'], status: 'OK', summary: '' }, error: null,
    });
    testExtractMock.mockResolvedValue({
      value: ['a', 'b', 'c'], value_type: 'array(3)', is_empty: false, error: null,
    });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_endpoint_path: '/live/toppages/v4',
          api_extract: { path: '$.items[*].id', fallback: null, fail_on_empty: false },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        nextStepType={{ type: 'BatchQuickPrompt' }}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testExtractMock).toHaveBeenCalled());
    expect(await screen.findByText('wf.apicall.nextStepBatchOk')).toBeInTheDocument();
  });

  it('banner warns with the resolved type when next step is Batch and data is scalar', async () => {
    testApiCallMock.mockResolvedValue({
      success: true, duration_ms: 5,
      envelope: { data: 42, status: 'OK', summary: '' }, error: null,
    });
    testExtractMock.mockResolvedValue({
      value: 42, value_type: 'number', is_empty: false, error: null,
    });
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_endpoint_path: '/live/toppages/v4',
          api_extract: { path: '$.total', fallback: null, fail_on_empty: false },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        nextStepType={{ type: 'BatchQuickPrompt' }}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf\.apicall\.testBtn/ }));
    await waitFor(() => expect(testExtractMock).toHaveBeenCalled());
    // The warning banner quotes the resolved value_type ("number" here).
    const banner = await screen.findByRole('alert');
    expect(banner.textContent).toContain('wf.apicall.nextStepBatchMismatch');
    expect(banner.textContent).toContain('number');
  });

  it('banner does not appear until a test has been run (no preview = no banner)', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({
          api_plugin_slug: 'chartbeat',
          api_endpoint_path: '/live/toppages/v4',
          api_extract: { path: '$.total', fallback: null, fail_on_empty: false },
        })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        nextStepType={{ type: 'BatchQuickPrompt' }}
        t={t}
      />,
    );
    // No test run yet → no banner (avoids a premature warning while the
    // user is still typing the path).
    expect(screen.queryByText(/nextStepBatch/)).not.toBeInTheDocument();
  });

  // ─── HTTP advanced (method / headers / body) — WU4 ──────────────

  it('method override picker propagates api_method on change', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { expanded: false }));
    // 0.8.6 (#62) — migrated to <Dropdown>: query by testId, click
    // trigger, click the POST option in the listbox.
    const methodTrigger = screen.getByTestId('wf-apicall-method-picker');
    expect(methodTrigger, 'method picker should be in DOM').toBeTruthy();
    fireEvent.click(methodTrigger);
    const postOption = screen.getByTestId('wf-apicall-method-picker-option-POST');
    fireEvent.click(postOption);
    expect(onChange).toHaveBeenCalledWith({ api_method: 'POST' });
  });

  it('headers editor adds a row propagating api_headers', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // Headers editor is rendered inline now (not behind the Advanced
    // toggle anymore), so no click needed to expand.
    // Two KeyValueEditors render (query + headers). Headers' value
    // placeholder is "application/json", uniquely identifying it.
    const headersValueInput = screen.getByPlaceholderText('application/json');
    const headersKeyInput = headersValueInput.previousElementSibling as HTMLInputElement;
    fireEvent.change(headersKeyInput, { target: { value: 'X-Custom' } });
    fireEvent.change(headersValueInput, { target: { value: 'value' } });
    const addBtn = headersValueInput.nextElementSibling as HTMLButtonElement;
    fireEvent.click(addBtn);
    expect(onChange).toHaveBeenCalledWith({ api_headers: { 'X-Custom': 'value' } });
  });

  it('body editor parses JSON on blur and propagates api_body', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { expanded: false }));
    const textarea = document.querySelector('textarea.wf-apicall-body-textarea') as HTMLTextAreaElement;
    expect(textarea, 'body textarea should be in DOM').toBeTruthy();
    fireEvent.change(textarea, { target: { value: '{"jql": "project = KR"}' } });
    fireEvent.blur(textarea);
    expect(onChange).toHaveBeenCalledWith({ api_body: { jql: 'project = KR' } });
  });

  it('body editor surfaces an inline error on invalid JSON without propagating', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { expanded: false }));
    const textarea = document.querySelector('textarea.wf-apicall-body-textarea') as HTMLTextAreaElement;
    onChange.mockClear();
    fireEvent.change(textarea, { target: { value: '{ broken json' } });
    fireEvent.blur(textarea);
    expect(onChange).not.toHaveBeenCalled();
    expect(document.body.textContent).toMatch(/JSON invalide/);
  });

  it('body editor empty input clears api_body to null', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    const onChange = vi.fn();
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat', api_body: { foo: 'bar' } })}
        onChange={onChange}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // The Advanced section auto-expands here because the step already
    // carries an `api_body` — no manual toggle click needed (and looking
    // for a `expanded: false` button would now fail since the toggle is
    // open from the start).
    const textarea = document.querySelector('textarea.wf-apicall-body-textarea') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '' } });
    fireEvent.blur(textarea);
    // Empty body → null (not "" or {}). Backend distinguishes "no body"
    // (GET) from "empty object body" (POST {}).
    expect(onChange).toHaveBeenCalledWith({ api_body: null });
  });

  it('advanced options toggle reveals timeout / retries / output_var inputs', () => {
    const plugins = [{ server: chartbeatServer, config: mkConfig('cfg-1', 'chartbeat') }];
    render(
      <ApiCallStepCard
        step={mkStep({ api_plugin_slug: 'chartbeat' })}
        onChange={() => {}}
        availableApiPlugins={plugins}
        projectId="proj-1"
        t={t}
      />,
    );
    // Collapsed by default.
    expect(screen.queryByText('wf.apicall.timeoutLabel')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { expanded: false }));
    expect(screen.getByText('wf.apicall.timeoutLabel')).toBeInTheDocument();
    expect(screen.getByText('wf.apicall.retriesLabel')).toBeInTheDocument();
    expect(screen.getByText('wf.apicall.outputVarLabel')).toBeInTheDocument();
  });
});
