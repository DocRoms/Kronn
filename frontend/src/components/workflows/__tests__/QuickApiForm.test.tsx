// QuickApiForm coverage (was 0%).
//
// QuickApiForm mirrors QuickPromptForm but the engine is HTTP, not LLM.
// It reuses ApiCallStepCard for the API config and treats a QuickApi as an
// ephemeral WorkflowStep of type ApiCall. The form:
//   - renders icon / name / project picker / description fields
//   - delegates the API config (plugin · endpoint · query · headers · body)
//     to ApiCallStepCard via the handleStepChange field router
//   - auto-syncs `{{var}}` tokens detected across the config into a
//     variables editor (label / placeholder / required / description)
//   - blocks Save until name + plugin + config + endpoint are all set, and
//     surfaces a hint listing what's still missing
//   - forwards the full CreateQuickApiRequest payload on save, with a
//     race-free guard + inline error surfacing on a rejected onSave
//
// Mirrors the sibling QuickPromptForm.bindings.test.tsx conventions:
// I18nProvider wrap + buildApiMock for ../../../lib/api.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act, cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import type { ReactElement } from 'react';
import type {
  McpServer,
  McpConfigDisplay,
  QuickApi,
} from '../../../types/generated';
import type { ApiPluginOption } from '../ApiCallStepCard';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { QuickApiForm } from '../QuickApiForm';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

// ─── Fixtures ───────────────────────────────────────────────────────────

const sampleServer: McpServer = {
  id: 'mcp-testapi',
  name: 'TestApi',
  description: 'A test API plugin',
  transport: 'ApiOnly',
  source: 'Registry',
  api_spec: {
    base_url: 'https://api.test.example',
    auth: 'None',
    endpoints: [
      { path: '/v1/things', method: 'GET', description: 'List things' },
      { path: '/v1/things/{id}', method: 'GET', description: 'Get a thing' },
    ],
  },
} as McpServer;

const sampleConfig: McpConfigDisplay = {
  id: 'cfg-1',
  server_id: 'mcp-testapi',
  server_name: 'TestApi',
  label: 'Default config',
  env_keys: [],
  env_masked: [],
  args_override: null,
  is_global: true,
  include_general: true,
  config_hash: 'hash',
  project_ids: ['proj-1'],
  project_names: ['ProjectAlpha'],
  secrets_broken: false,
  host_sync: 'None',
} as McpConfigDisplay;

const samplePlugins: ApiPluginOption[] = [
  { server: sampleServer, config: sampleConfig },
];

const sampleProjects = [
  { id: 'proj-1', name: 'ProjectAlpha' },
  { id: 'proj-2', name: 'ProjectBeta' },
];

// A fully-wired QuickApi for the edit-mode tests. Carries a `{{thingId}}`
// token in the endpoint path so the variables editor renders pre-filled.
const editApi: QuickApi = {
  id: 'qa-1',
  name: 'Fetch a thing',
  icon: '📦',
  description: 'Fetches one thing by id',
  project_id: 'proj-1',
  api_plugin_slug: 'mcp-testapi',
  api_config_id: 'cfg-1',
  api_endpoint_path: '/v1/things/{{thingId}}',
  api_method: 'GET',
  api_query: { format: 'json' },
  api_path_params: null,
  api_headers: { 'X-Trace': '{{traceId}}' },
  api_body: null,
  api_extract: { path: '$.data', fallback: null, fail_on_empty: false },
  api_pagination: null,
  api_timeout_ms: null,
  api_max_retries: null,
  variables: [
    { name: 'thingId', label: 'Thing id', placeholder: '42', description: null, required: true },
  ],
  profile_ids: ['p1'],
  directive_ids: ['d1'],
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
} as QuickApi;

function renderForm(props?: Partial<Parameters<typeof QuickApiForm>[0]>) {
  const onSave = vi.fn().mockResolvedValue(undefined);
  const onCancel = vi.fn();
  return {
    onSave,
    onCancel,
    ...wrap(
      <QuickApiForm
        projects={sampleProjects as Parameters<typeof QuickApiForm>[0]['projects']}
        availableApiPlugins={samplePlugins}
        installedAgents={[]}
        onSave={onSave}
        onCancel={onCancel}
        {...props}
      />,
    ),
  };
}

// The form's two `.wf-input` text fields are [0] = icon, [1] = name.
const iconInput = () => document.querySelectorAll('input.wf-input')[0] as HTMLInputElement;
const nameInput = () => document.querySelectorAll('input.wf-input')[1] as HTMLInputElement;
const saveBtn = () => document.querySelector('.wf-create-btn') as HTMLButtonElement;
// Plugin picker = the only <select> in ApiCallStepCard's empty/inline state.
const pluginSelect = () => document.querySelector('.wf-apicall-pickers select') as HTMLSelectElement;
const endpointInput = () =>
  document.querySelector('.wf-apicall-pickers input[type="text"]') as HTMLInputElement;

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { cleanup(); });

describe('QuickApiForm — initial render (new)', () => {
  it('renders the new-QA heading and the icon default', () => {
    renderForm();
    expect(screen.getByText(/Nouveau Quick API|New Quick API|Nuevo Quick API/)).toBeInTheDocument();
    expect(iconInput().value).toBe('🔌');
  });

  it('embeds the ApiCallStepCard (region) when plugins are available', () => {
    renderForm();
    expect(document.querySelector('.wf-apicall-card')).not.toBeNull();
    expect(pluginSelect()).not.toBeNull();
  });

  it('shows the API-not-supported empty state when no plugins are wired', () => {
    renderForm({ availableApiPlugins: [] });
    expect(document.querySelector('.wf-apicall-card-empty')).not.toBeNull();
  });

  it('Save is disabled and the blocked-hint lists every missing field', () => {
    renderForm();
    expect(saveBtn().disabled).toBe(true);
    const hint = document.querySelector('.text-ghost.mb-2') as HTMLElement;
    expect(hint).not.toBeNull();
    // All four required fields are empty on a fresh form.
    expect(hint.textContent ?? '').toMatch(/,/); // multiple fields joined
  });

  it('renders the no-variables hint when nothing is detected yet', () => {
    renderForm();
    // The hint paragraph is the `.text-2xs.text-ghost` block.
    expect(document.querySelector('.text-2xs.text-ghost')).not.toBeNull();
    // No variable rows.
    expect(document.querySelector('.qp-var-row')).toBeNull();
  });
});

describe('QuickApiForm — field edits', () => {
  it('edits the icon and name fields', () => {
    renderForm();
    fireEvent.change(iconInput(), { target: { value: '🚀' } });
    fireEvent.change(nameInput(), { target: { value: 'My QA' } });
    expect(iconInput().value).toBe('🚀');
    expect(nameInput().value).toBe('My QA');
  });

  it('edits the description textarea', () => {
    renderForm();
    const ta = document.querySelector('textarea.wf-textarea') as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: 'Does a thing' } });
    expect(ta.value).toBe('Does a thing');
  });

  it('changes the project via the Dropdown picker', () => {
    renderForm();
    const trigger = screen.getByTestId('qa-project-picker');
    fireEvent.click(trigger);
    fireEvent.click(screen.getByText('ProjectBeta'));
    // The selected label surfaces on the trigger.
    expect(screen.getByTestId('qa-project-picker').textContent).toMatch(/ProjectBeta/);
  });
});

describe('QuickApiForm — API config wiring (via ApiCallStepCard)', () => {
  it('selecting a plugin then typing an endpoint flips Save enabled', async () => {
    renderForm();
    // name
    fireEvent.change(nameInput(), { target: { value: 'My QA' } });
    // plugin (writes api_plugin_slug + api_config_id)
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    // endpoint
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things' } });

    await waitFor(() => expect(saveBtn().disabled).toBe(false));
  });

  it('the endpoint input is disabled until a plugin is selected', () => {
    renderForm();
    expect(endpointInput().disabled).toBe(true);
  });
});

describe('QuickApiForm — variables auto-sync', () => {
  it('detects a {{var}} typed into the endpoint and renders an editable row', async () => {
    renderForm();
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things/{{thingId}}' } });

    await waitFor(() => expect(document.querySelector('.qp-var-row')).not.toBeNull());
    expect(screen.getByText('{{thingId}}')).toBeInTheDocument();
  });

  it('edits a detected variable label / placeholder / required / description', async () => {
    renderForm();
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things/{{thingId}}' } });
    await waitFor(() => expect(document.querySelector('.qp-var-row')).not.toBeNull());

    const row = document.querySelector('.qp-var-row') as HTMLElement;
    const [labelInput, placeholderInput] = row.querySelectorAll('input.wf-input');
    const checkbox = row.querySelector('input[type="checkbox"]') as HTMLInputElement;
    const descInput = row.querySelectorAll('input.wf-input')[2] as HTMLInputElement;

    fireEvent.change(labelInput, { target: { value: 'The id' } });
    fireEvent.change(placeholderInput, { target: { value: 'e.g. 42' } });
    fireEvent.click(checkbox); // toggle required off
    fireEvent.change(descInput, { target: { value: 'thing identifier' } });

    expect((labelInput as HTMLInputElement).value).toBe('The id');
    expect((placeholderInput as HTMLInputElement).value).toBe('e.g. 42');
    expect(checkbox.checked).toBe(false);
    expect(descInput.value).toBe('thing identifier');
  });

  it('removing the {{var}} from the endpoint drops the variable row', async () => {
    renderForm();
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things/{{thingId}}' } });
    await waitFor(() => expect(document.querySelector('.qp-var-row')).not.toBeNull());

    fireEvent.change(endpointInput(), { target: { value: '/v1/things' } });
    await waitFor(() => expect(document.querySelector('.qp-var-row')).toBeNull());
  });
});

describe('QuickApiForm — save', () => {
  it('does not save while a required field is missing (button stays disabled)', () => {
    const { onSave } = renderForm();
    // No name → disabled. Clicking a disabled button does nothing.
    fireEvent.click(saveBtn());
    expect(onSave).not.toHaveBeenCalled();
  });

  it('forwards the full payload on save once all required fields are set', async () => {
    const { onSave } = renderForm();
    fireEvent.change(iconInput(), { target: { value: '🚀' } });
    fireEvent.change(nameInput(), { target: { value: 'My QA' } });
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things' } });
    await waitFor(() => expect(saveBtn().disabled).toBe(false));

    await act(async () => { fireEvent.click(saveBtn()); });
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));

    const payload = onSave.mock.calls[0][0];
    expect(payload.name).toBe('My QA');
    expect(payload.icon).toBe('🚀');
    expect(payload.api_plugin_slug).toBe('mcp-testapi');
    expect(payload.api_config_id).toBe('cfg-1');
    expect(payload.api_endpoint_path).toBe('/v1/things');
    // Empty project_id maps to null per the form contract.
    expect(payload.project_id).toBeNull();
  });

  it('surfaces a rejected onSave inline as an error alert', async () => {
    const onSave = vi.fn().mockRejectedValue(new Error('name must be 1-200 chars'));
    renderForm({ onSave });
    fireEvent.change(nameInput(), { target: { value: 'My QA' } });
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things' } });
    await waitFor(() => expect(saveBtn().disabled).toBe(false));

    await act(async () => { fireEvent.click(saveBtn()); });

    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/name must be 1-200 chars/),
    );
    // Save button is re-enabled after the failed attempt (saving flag cleared).
    await waitFor(() => expect(saveBtn().disabled).toBe(false));
  });

  it('stringifies a non-Error rejection value', async () => {
    const onSave = vi.fn().mockRejectedValue('plain string failure');
    renderForm({ onSave });
    fireEvent.change(nameInput(), { target: { value: 'My QA' } });
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things' } });
    await waitFor(() => expect(saveBtn().disabled).toBe(false));

    await act(async () => { fireEvent.click(saveBtn()); });
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/plain string failure/),
    );
  });

  it('guards against a double-click double-save (race-free ref)', async () => {
    // Hold onSave pending so the second synchronous click hits the guard.
    let resolveSave: (() => void) | null = null;
    const onSave = vi.fn().mockImplementation(
      () => new Promise<void>(res => { resolveSave = () => res(); }),
    );
    renderForm({ onSave });
    fireEvent.change(nameInput(), { target: { value: 'My QA' } });
    fireEvent.change(pluginSelect(), { target: { value: 'cfg-1' } });
    await waitFor(() => expect(endpointInput().disabled).toBe(false));
    fireEvent.change(endpointInput(), { target: { value: '/v1/things' } });
    await waitFor(() => expect(saveBtn().disabled).toBe(false));

    await act(async () => {
      fireEvent.click(saveBtn());
      fireEvent.click(saveBtn()); // second click while the first is in-flight
    });
    expect(onSave).toHaveBeenCalledTimes(1);
    await act(async () => { resolveSave?.(); });
  });

  it('fires onCancel when the close button is clicked', () => {
    const { onCancel } = renderForm();
    const closeBtn = document.querySelector('.wf-icon-btn') as HTMLButtonElement;
    fireEvent.click(closeBtn);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});

describe('QuickApiForm — edit mode', () => {
  it('pre-fills name, icon, description and the project from editApi', () => {
    renderForm({ editApi });
    expect(nameInput().value).toBe('Fetch a thing');
    expect(iconInput().value).toBe('📦');
    const ta = document.querySelector('textarea.wf-textarea') as HTMLTextAreaElement;
    expect(ta.value).toBe('Fetches one thing by id');
    // Heading shows the existing name (edit mode), not the "new" label.
    expect(screen.getByText('Fetch a thing')).toBeInTheDocument();
  });

  it('renders the pre-existing detected variables (preserving the saved label)', () => {
    renderForm({ editApi });
    expect(document.querySelector('.qp-var-row')).not.toBeNull();
    // thingId (endpoint) + traceId (header) are both detected.
    expect(screen.getByText('{{thingId}}')).toBeInTheDocument();
    expect(screen.getByText('{{traceId}}')).toBeInTheDocument();
    // The user-set label on thingId is preserved through the auto-sync.
    const labelInputs = document.querySelectorAll('.qp-var-row input.wf-input');
    expect((labelInputs[0] as HTMLInputElement).value).toBe('Thing id');
  });

  it('Save is enabled immediately in edit mode and round-trips the config', async () => {
    const { onSave } = renderForm({ editApi });
    await waitFor(() => expect(saveBtn().disabled).toBe(false));

    await act(async () => { fireEvent.click(saveBtn()); });
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));

    const payload = onSave.mock.calls[0][0];
    expect(payload.name).toBe('Fetch a thing');
    expect(payload.api_endpoint_path).toBe('/v1/things/{{thingId}}');
    expect(payload.api_method).toBe('GET');
    expect(payload.api_query).toEqual({ format: 'json' });
    expect(payload.project_id).toBe('proj-1');
    expect(payload.api_extract).toEqual({ path: '$.data', fallback: null, fail_on_empty: false });
    // Bindings round-trip unchanged (QA picker UI lives in QuickPromptForm).
    expect(payload.profile_ids).toEqual(['p1']);
    expect(payload.directive_ids).toEqual(['d1']);
  });
});
