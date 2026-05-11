// Tests for ApiCallAiHelper — the chat-bubble that suggests endpoint /
// query / extract for an ApiCall step.
//
// Scope: pure helpers (KRONN:APPLY parsing, applyToStep field allowlist),
// trigger-button rendering, and the agent-picker shortcut when only one
// agent is installed.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { McpServer, WorkflowStep } from '../../../types/generated';

const { createMock } = vi.hoisted(() => ({
  createMock: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  discussions: {
    create: createMock as never,
  },
}));

import { ApiCallAiHelper, applyToStep, parseApplyBlocks, buildContextBlock } from '../ApiCallAiHelper';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

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

const fakeServer: McpServer = {
  id: 'chartbeat',
  name: 'Chartbeat',
  description: '',
  transport: 'ApiOnly',
  source: 'Registry',
  api_spec: {
    base_url: 'https://api.chartbeat.com',
    auth: { ApiKeyQuery: { param_name: 'apikey', env_key: 'CB_KEY' } },
    endpoints: [{ path: '/live/toppages/v4', method: 'GET', description: 'Top pages' }],
    docs_url: null,
    config_keys: [],
  },
};

describe('buildContextBlock — fresh context attached to every message', () => {
  it('lists the current step state (api/endpoint/query/extract)', () => {
    const block = buildContextBlock(
      fakeServer,
      mkStep({
        api_endpoint_path: '/live/toppages/v4',
        api_query: { host: 'www.euronews.com', limit: '5' },
        api_extract: { path: '$.pages[*].title', fallback: null, fail_on_empty: false },
      }),
      undefined,
      undefined,
      t,
    );
    expect(block).toContain('API : Chartbeat');
    expect(block).toContain('endpoint : /live/toppages/v4');
    expect(block).toContain('"host":"www.euronews.com"');
    expect(block).toContain('extract  : $.pages[*].title');
  });

  it('embeds the last test error verbatim (so the agent can debug "why 400")', () => {
    const block = buildContextBlock(
      fakeServer,
      mkStep({ api_endpoint_path: '/live/toppages/v4' }),
      undefined,
      'HTTP 400 Bad Request — Missing required parameter `host`',
      t,
    );
    expect(block).toContain('wf.apicall.helper.sys.ctxLastFail');
    expect(block).toContain('Missing required parameter `host`');
  });

  it('embeds the last test response when there is no error', () => {
    const block = buildContextBlock(
      fakeServer,
      mkStep({ api_endpoint_path: '/live/toppages/v4' }),
      { pages: [{ title: 'Article A', path: '/a' }] },
      undefined,
      t,
    );
    expect(block).toContain('wf.apicall.helper.sys.ctxLastOk');
    expect(block).toContain('"Article A"');
  });

  it('omits both blocks when no test has run yet', () => {
    const block = buildContextBlock(fakeServer, mkStep(), undefined, undefined, t);
    expect(block).not.toContain('wf.apicall.helper.sys.ctxLast');
  });
});

describe('plugin tips registry', () => {
  it('returns Chartbeat-specific lore (host pitfall) for slug "chartbeat"', async () => {
    const { tipsForSlug } = await import('../apiCallPluginTips');
    const tips = tipsForSlug('chartbeat');
    expect(tips).not.toBeNull();
    expect(tips!.body).toContain('host');
    expect(tips!.body).toMatch(/Settings.*Sites/);
  });

  it('returns null for unknown plugin slugs (prompt simply omits the section)', async () => {
    const { tipsForSlug } = await import('../apiCallPluginTips');
    expect(tipsForSlug(null)).toBeNull();
    expect(tipsForSlug('made-up-plugin')).toBeNull();
  });

  it('returns GitHub-specific lore (path placeholder pitfall) for slug "mcp-github"', async () => {
    // GitHub's hybrid plugin reuses the MCP id "mcp-github" — the tips
    // registry must key on the same slug so the AI helper picks them up
    // when the user selects the GitHub plugin in the wizard.
    const { tipsForSlug } = await import('../apiCallPluginTips');
    const tips = tipsForSlug('mcp-github');
    expect(tips).not.toBeNull();
    // The most-bitten footgun: agents forgetting to substitute {owner}/
    // {repo} placeholders in the path. The tip MUST mention this.
    expect(tips!.body).toMatch(/placeholders.*owner.*repo|\{owner\}.*\{repo\}/);
    expect(tips!.body).toContain('/user');
    expect(tips!.docsUrl).toContain('docs.github.com/en/rest');
  });
});

describe('parseApplyBlocks — KRONN:APPLY parser', () => {
  it('extracts a single complete block from prose', () => {
    const text = 'Voici une suggestion :\n\nKRONN:APPLY\n```json\n{ "endpoint": "/x" }\n```\nVoilà.';
    const blocks = parseApplyBlocks(text);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].parsed).toEqual({ endpoint: '/x' });
    expect(blocks[0].applied).toBe(false);
  });

  it('extracts multiple blocks in one streaming message', () => {
    // Agents sometimes propose two variants in the same reply (e.g. one
    // without filter, one with `host=…`). Both must surface as separate
    // Apply cards so the user can pick.
    const text = `option A:
KRONN:APPLY
\`\`\`json
{ "endpoint": "/a" }
\`\`\`
option B:
KRONN:APPLY
\`\`\`json
{ "endpoint": "/b", "extract": "$.b" }
\`\`\``;
    const blocks = parseApplyBlocks(text);
    expect(blocks).toHaveLength(2);
    expect(blocks[0].parsed).toEqual({ endpoint: '/a' });
    expect(blocks[1].parsed).toEqual({ endpoint: '/b', extract: '$.b' });
  });

  it('silently skips malformed JSON (incoming streaming chunks are partial)', () => {
    // Mid-stream, the closing brace may not have arrived yet. The parser
    // must not throw; the block will surface on the next chunk once the
    // JSON is well-formed.
    const text = 'KRONN:APPLY\n```json\n{ "endpoint": "/x", "query": {\n```';
    expect(() => parseApplyBlocks(text)).not.toThrow();
    expect(parseApplyBlocks(text)).toEqual([]);
  });

  it('returns [] when no KRONN:APPLY marker is present', () => {
    expect(parseApplyBlocks('Just chatting, no suggestion here.')).toEqual([]);
  });
});

describe('applyToStep — KRONN:APPLY field allowlist', () => {
  it('maps endpoint/method/query/headers/body/extract', () => {
    const step = mkStep();
    const updates = applyToStep({
      endpoint: '/live/toppages/v4',
      method: 'get',
      query: { host: 'www.euronews.com', limit: 5 },
      headers: { 'X-Test': 'yes' },
      body: { foo: 'bar' },
      extract: '$.pages[*].path',
    }, step);
    expect(updates.api_endpoint_path).toBe('/live/toppages/v4');
    expect(updates.api_method).toBe('GET');
    expect(updates.api_query).toEqual({ host: 'www.euronews.com', limit: '5' });
    expect(updates.api_headers).toEqual({ 'X-Test': 'yes' });
    expect(updates.api_body).toBe('{"foo":"bar"}');
    expect(updates.api_extract?.path).toBe('$.pages[*].path');
  });

  it('preserves extract.fail_on_empty when only the path changes', () => {
    const step = mkStep({
      api_extract: { path: '$.old', fallback: 'X', fail_on_empty: true },
    });
    const updates = applyToStep({ extract: '$.new' }, step);
    expect(updates.api_extract).toEqual({
      path: '$.new',
      fallback: 'X',
      fail_on_empty: true,
    });
  });

  it('ignores out-of-allowlist fields (no agent prompt_template hijack)', () => {
    const step = mkStep();
    const updates = applyToStep({
      // These would be a hallucinated suggestion targeting a sensitive field;
      // applyToStep must drop them silently.
      agent: 'Codex',
      prompt_template: 'haha',
      api_timeout_ms: 999,
      // ─── Allowed companion ───
      endpoint: '/safe',
    } as Record<string, unknown>, step);
    expect(updates).toEqual({ api_endpoint_path: '/safe' });
  });

  it('strips auth-managed query params (apikey already injected by backend)', () => {
    // Chartbeat declares ApiKeyQuery with `param_name: 'apikey'`. An agent
    // hallucinating `apikey: 'VOTRE_API_KEY'` would shadow the real value
    // — applyToStep must drop it silently.
    const step = mkStep();
    const updates = applyToStep(
      { query: { host: 'fr.euronews.com', apikey: 'VOTRE_API_KEY' } },
      step,
      fakeServer,
    );
    expect(updates.api_query).toEqual({ host: 'fr.euronews.com' });
  });

  it('keeps a User-Agent header (Bearer plugin) — regression for the GitHub case', () => {
    // User reported "applied User-Agent suggestion, nothing happened".
    // The bug turned out to be UI (Headers editor hidden in collapsed
    // Advanced section), but `applyToStep` itself MUST also be a clean
    // pass-through here: the Bearer auth managed header is `Authorization`
    // (case-insensitive lookup), so `User-Agent` survives untouched.
    const githubServer: McpServer = {
      ...fakeServer,
      id: 'mcp-github',
      name: 'GitHub',
      api_spec: { ...fakeServer.api_spec!, auth: { Bearer: { env_key: 'GITHUB_PERSONAL_ACCESS_TOKEN' } } },
    };
    const updates = applyToStep(
      { headers: { 'User-Agent': 'Kronn-Workflow/1.0' } },
      mkStep(),
      githubServer,
    );
    expect(updates.api_headers).toEqual({ 'User-Agent': 'Kronn-Workflow/1.0' });
    // Other fields untouched (we only suggested headers).
    expect(updates.api_endpoint_path).toBeUndefined();
    expect(updates.api_query).toBeUndefined();
  });

  it('drops auth-managed headers (Bearer / ApiKeyHeader) from a suggestion', () => {
    const bearerServer: McpServer = {
      ...fakeServer,
      api_spec: { ...fakeServer.api_spec!, auth: { Bearer: { env_key: 'JIRA_TOKEN' } } },
    };
    const updates = applyToStep(
      { headers: { 'X-Trace': 'yes', Authorization: 'Bearer hallucinated-token' } },
      mkStep(),
      bearerServer,
    );
    // Authorization is auto-injected; only the user-supplied header survives.
    expect(updates.api_headers).toEqual({ 'X-Trace': 'yes' });
  });
});

describe('ApiCallAiHelper — UI rendering', () => {
  beforeEach(() => {
    createMock.mockReset();
  });

  it('disables the trigger when no API plugin is selected (no spec to read)', () => {
    // Without an API selected, the agent has no endpoints list, no auth
    // info, no plugin tips — its advice degenerates to "go pick an API".
    // The trigger must be hard-disabled rather than letting the user open
    // an empty helper.
    render(
      <ApiCallAiHelper
        step={mkStep()}
        onApply={() => {}}
        selectedServer={null}
        projectId="proj-1"
        installedAgents={['ClaudeCode']}
        t={t}
      />,
    );
    const btn = screen.getByRole('button', { name: /wf.apicall.helper.trigger/ });
    expect((btn as HTMLButtonElement).disabled).toBe(true);
  });

  it('hides trigger when no installed agents are available', () => {
    // Hidden by parent (`installedAgents.length > 0` guard in
    // ApiCallStepCard) — here we still render the helper but expect the
    // button to surface a "no agents" error if clicked.
    render(
      <ApiCallAiHelper
        step={mkStep()}
        onApply={() => {}}
        selectedServer={fakeServer}
        projectId="proj-1"
        installedAgents={[]}
        t={t}
      />,
    );
    const btn = screen.getByRole('button', { name: /wf.apicall.helper.trigger/ });
    expect(btn).toBeTruthy();
  });

  it('opens chat directly with the first installed agent, picker available via header dropdown', () => {
    // 0.8.1 UX: no separate picking-agent phase. Clicking the trigger
    // opens the chat bubble straight away with the first installed agent.
    // Switching to another agent happens via the header dropdown.
    createMock.mockResolvedValue({ id: 'disc-1', title: 'helper' });
    render(
      <ApiCallAiHelper
        step={mkStep()}
        onApply={() => {}}
        selectedServer={fakeServer}
        projectId="proj-1"
        installedAgents={['ClaudeCode', 'Codex', 'GeminiCli']}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.trigger/ }));
    // Discussion is created with the first agent (ClaudeCode).
    expect(createMock).toHaveBeenCalledTimes(1);
    expect(createMock.mock.calls[0][0].agent).toBe('ClaudeCode');
    // The header dropdown trigger surfaces the active agent's label.
    expect(screen.getByText('Claude Code')).toBeTruthy();
  });

  it('skips the picker when only one agent is installed', () => {
    // With a single installed agent we should jump directly to the chat
    // bubble — no point asking the user to pick from a list of one.
    createMock.mockResolvedValue({ id: 'disc-1', title: 'helper' });
    render(
      <ApiCallAiHelper
        step={mkStep()}
        onApply={() => {}}
        selectedServer={fakeServer}
        projectId="proj-1"
        installedAgents={['ClaudeCode']}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.trigger/ }));
    expect(createMock).toHaveBeenCalledTimes(1);
    expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      project_id: 'proj-1',
      agent: 'ClaudeCode',
    }));
  });

  it('still creates the helper discussion when no project is selected', () => {
    // The AI helper is project-agnostic: it just needs the API spec
    // (selectedServer). Forcing a project pick would be hostile when the
    // user hasn't even configured the wizard's first page. `project_id`
    // accepts null on the backend `CreateDiscussionRequest`.
    createMock.mockResolvedValue({ id: 'disc-no-project', title: 'helper' });
    render(
      <ApiCallAiHelper
        step={mkStep()}
        onApply={() => {}}
        selectedServer={fakeServer}
        projectId={null}
        installedAgents={['ClaudeCode']}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.trigger/ }));
    expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      project_id: null,
      agent: 'ClaudeCode',
    }));
  });
});
