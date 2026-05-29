// Flow tests for ApiCallAiHelper — exercises the chat lifecycle that the
// base test file (ApiCallAiHelper.test.tsx) leaves uncovered:
//   - send a message → sendMessageStream wiring (enriched context prepended)
//   - streaming chunks land in the assistant bubble, the typing indicator
//     shows, and a streamed KRONN:APPLY block surfaces an Apply card
//   - clicking Apply forwards a mapped Partial<WorkflowStep> to onApply
//   - the onError stream branch surfaces a visible error
//   - stop button aborts + calls discussions.stop
//   - minimize / restore / close lifecycle
//   - switch agent kills the old discussion and primes a new one
//   - empty/whitespace input is guarded (no stream fired)
//   - starter chips prefill the input
//
// Conventions mirror DebugSection.test.tsx (hoisted vi.fn mocks, act +
// waitFor) and the sibling base test (buildApiMock + fireEvent).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { McpServer, WorkflowStep } from '../../../types/generated';

const { createMock, streamMock, deleteMock, stopMock } = vi.hoisted(() => ({
  createMock: vi.fn(),
  streamMock: vi.fn(),
  deleteMock: vi.fn(),
  stopMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  discussions: {
    create: createMock as never,
    sendMessageStream: streamMock as never,
    delete: deleteMock as never,
    stop: stopMock as never,
  },
}));

import { ApiCallAiHelper } from '../ApiCallAiHelper';
import type { ApiCallAiHelperProps } from '../ApiCallAiHelper';

const t: ApiCallAiHelperProps['t'] = (key, ...args) =>
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

function renderHelper(over: Partial<ApiCallAiHelperProps> = {}) {
  const onApply = vi.fn<ApiCallAiHelperProps['onApply']>();
  const utils = render(
    <ApiCallAiHelper
      step={mkStep()}
      onApply={onApply}
      selectedServer={fakeServer}
      projectId="proj-1"
      installedAgents={['ClaudeCode', 'Codex']}
      t={t}
      {...over}
    />,
  );
  return { onApply, ...utils };
}

/** Open the chat bubble (create() resolves to a discussion id). */
async function openChat(over: Partial<ApiCallAiHelperProps> = {}) {
  const utils = renderHelper(over);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.trigger/ }));
  });
  // create() resolves on a microtask; wait for the discussion id to settle.
  await waitFor(() => expect(createMock).toHaveBeenCalled());
  return utils;
}

beforeEach(() => {
  createMock.mockReset().mockResolvedValue({ id: 'disc-1', title: 'helper' });
  streamMock.mockReset();
  deleteMock.mockReset().mockResolvedValue(undefined);
  stopMock.mockReset().mockResolvedValue({ cancelled: true });
});

afterEach(() => {
  cleanup();
});

describe('ApiCallAiHelper — startWithAgent', () => {
  it('surfaces an error when create() rejects', async () => {
    createMock.mockRejectedValueOnce(new Error('backend down'));
    renderHelper();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.trigger/ }));
    });
    // The phase still flips to chatting (setError fires after setPhase), so
    // the error renders inside the bubble.
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('backend down'));
  });

  it('shows the welcome state with 3 starter chips before any message', async () => {
    await openChat();
    expect(screen.getByText(/wf.apicall.helper.welcome/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /wf.apicall.helper.starter.build/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: /wf.apicall.helper.starter.endpoint/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: /wf.apicall.helper.starter.body/ })).toBeTruthy();
  });

  it('a starter chip prefills the textarea', async () => {
    await openChat();
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.starter.build/ }));
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    expect(textarea.value).toContain('wf.apicall.helper.starter.buildPrompt');
  });
});

describe('ApiCallAiHelper — send message', () => {
  it('guards empty / whitespace-only input (no stream fired)', async () => {
    await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '   ' } });
    // Send button is disabled on blank input; Enter is the only path.
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(streamMock).not.toHaveBeenCalled();
  });

  it('sends the typed text via sendMessageStream with an enriched context block', async () => {
    await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'build the toppages call' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(streamMock).toHaveBeenCalledTimes(1));
    const [discId, req] = streamMock.mock.calls[0];
    expect(discId).toBe('disc-1');
    // The user's question is embedded after the fresh context block.
    expect(req.content).toContain('build the toppages call');
    expect(req.content).toContain('API : Chartbeat');
    // The user bubble shows the raw text (not the enriched payload).
    expect(screen.getByText('build the toppages call')).toBeTruthy();
    // Input cleared after send.
    expect(textarea.value).toBe('');
  });

  it('streams chunks into the assistant bubble and surfaces a KRONN:APPLY card', async () => {
    // Drive the stream synchronously: invoke onChunk a couple times then onDone.
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk('Try this endpoint.\n');
      onChunk('KRONN:APPLY\n```json\n{ "endpoint": "/live/toppages/v4", "method": "get" }\n```');
      onDone();
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(screen.getByText(/Try this endpoint/)).toBeTruthy());
    // The KRONN:APPLY block is rendered as a card, not raw fenced JSON.
    expect(screen.getByText(/wf.apicall.helper.suggestion/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /wf.apicall.helper.apply$/ })).toBeTruthy();
  });

  it('Apply forwards a mapped Partial<WorkflowStep> to onApply and disables the button', async () => {
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk('KRONN:APPLY\n```json\n{ "endpoint": "/live/toppages/v4", "method": "get" }\n```');
      onDone();
      return Promise.resolve();
    });
    const { onApply } = await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    const applyBtn = await screen.findByRole('button', { name: /wf.apicall.helper.apply$/ });
    fireEvent.click(applyBtn);
    expect(onApply).toHaveBeenCalledTimes(1);
    expect(onApply.mock.calls[0][0]).toEqual({
      api_endpoint_path: '/live/toppages/v4',
      api_method: 'GET',
    });
    // The card flips to the applied state.
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /wf.apicall.helper.applied/ })).toBeTruthy(),
    );
  });

  it('surfaces the onError stream branch as a visible error', async () => {
    streamMock.mockImplementation((_id, _req, _onChunk, _onDone, onError) => {
      onError('stream exploded');
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(screen.getByText('stream exploded')).toBeTruthy());
  });

  it('Shift+Enter does NOT send (newline insertion)', async () => {
    await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'multi' } });
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: true });
    expect(streamMock).not.toHaveBeenCalled();
  });
});

describe('ApiCallAiHelper — stop while streaming', () => {
  it('renders a stop button mid-stream and calls discussions.stop', async () => {
    // Keep the stream "open" — never call onDone so the component stays in
    // the streaming state and renders the stop button.
    streamMock.mockImplementation((_id, _req, onChunk) => {
      onChunk('thinking…');
      return new Promise(() => {});
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/wf.apicall.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    const stopBtn = await screen.findByRole('button', { name: /wf.apicall.helper.stop/ });
    fireEvent.click(stopBtn);
    await waitFor(() => expect(stopMock).toHaveBeenCalledWith('disc-1'));
  });
});

describe('ApiCallAiHelper — minimize / restore / close', () => {
  it('minimizes to a restore pill and restores the bubble', async () => {
    await openChat();
    expect(screen.getByRole('dialog')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.minimize/ }));
    expect(screen.queryByRole('dialog')).toBeNull();
    const restore = screen.getByRole('button', { name: /wf.apicall.helper.restore/ });
    fireEvent.click(restore);
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it('close tears down the discussion (delete) and returns to the trigger-only phase', async () => {
    await openChat();
    fireEvent.click(screen.getByRole('button', { name: /wf.apicall.helper.close/ }));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith('disc-1'));
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('ApiCallAiHelper — agent switch', () => {
  it('opens the dropdown and switching agents kills the old disc + creates a new one', async () => {
    await openChat();
    const headerTrigger = screen.getAllByRole('button').find(
      btn => btn.getAttribute('aria-haspopup') === 'listbox',
    )!;
    fireEvent.click(headerTrigger);
    expect(screen.getByRole('listbox')).toBeTruthy();
    createMock.mockClear();
    // Switch to Codex (a different installed agent).
    fireEvent.click(screen.getByRole('option', { name: /Codex/ }));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith('disc-1'));
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock.mock.calls[0][0].agent).toBe('Codex');
  });
});
