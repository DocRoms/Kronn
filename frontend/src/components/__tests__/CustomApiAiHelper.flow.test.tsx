// Flow tests for CustomApiAiHelper — exercises the chat lifecycle that the
// base test file (CustomApiAiHelper.test.tsx) leaves uncovered:
//   - send a message → sendMessageStream wiring (form context prepended)
//   - streaming chunks land in the assistant bubble + a streamed KRONN:APPLY
//     block surfaces an Apply card
//   - clicking Apply forwards a mapped Partial<CustomApiPayload> to onApply
//   - the onError stream branch surfaces a visible error
//   - create() rejection surfaces an error
//   - empty agents list surfaces the "no agents" inline error
//   - stop button aborts + calls discussions.stop
//   - minimize / restore / close lifecycle
//   - switch agent kills the old discussion and primes a new one
//   - empty input is guarded (no stream fired)
//
// Conventions mirror the sibling base test (inline vi.mock of lib/api) and
// DebugSection.test.tsx (act + waitFor). NOTE: unlike the base test we use
// REAL timers — the lifecycle assertions wait on real microtasks.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';

const { createMock, streamMock, deleteMock, stopMock } = vi.hoisted(() => ({
  createMock: vi.fn(),
  streamMock: vi.fn(),
  deleteMock: vi.fn(),
  stopMock: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussions: {
    create: createMock,
    sendMessageStream: streamMock,
    runAgent: vi.fn(),
    delete: deleteMock,
    stop: stopMock,
  },
}));

import { CustomApiAiHelper } from '../CustomApiAiHelper';
import type { CustomApiAiHelperProps } from '../CustomApiAiHelper';
import type { AgentType } from '../../types/generated';

const t: CustomApiAiHelperProps['t'] = (key, ...args) =>
  args.length === 0 ? key : `${key}(${args.join(',')})`;

const baseSnapshot: CustomApiAiHelperProps['formSnapshot'] = {
  name: 'MyAPI',
  base_url: 'https://x.test',
  description: '',
  docs_url: '',
  fields: [{ label: '', value: '' }],
  endpoints: [],
};

function renderHelper(over: Partial<CustomApiAiHelperProps> = {}) {
  const onApply = vi.fn<CustomApiAiHelperProps['onApply']>();
  const installedAgents: AgentType[] = over.installedAgents ?? ['ClaudeCode', 'Codex'];
  const utils = render(
    <CustomApiAiHelper
      formSnapshot={over.formSnapshot ?? baseSnapshot}
      onApply={onApply}
      installedAgents={installedAgents}
      t={t}
      {...over}
    />,
  );
  return { onApply, ...utils };
}

async function openChat(over: Partial<CustomApiAiHelperProps> = {}) {
  const utils = renderHelper(over);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.trigger/ }));
  });
  await waitFor(() => expect(createMock).toHaveBeenCalled());
  return utils;
}

beforeEach(() => {
  createMock.mockReset().mockResolvedValue({ id: 'disc-c', title: 'helper' });
  streamMock.mockReset();
  deleteMock.mockReset().mockResolvedValue(undefined);
  stopMock.mockReset().mockResolvedValue({ cancelled: true });
});

afterEach(() => {
  cleanup();
});

describe('CustomApiAiHelper — startWithAgent', () => {
  it('surfaces an error when create() rejects', async () => {
    createMock.mockRejectedValueOnce(new Error('backend down'));
    renderHelper();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.trigger/ }));
    });
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('backend down'));
  });

  it('shows a "no agents" inline error when none are installed (no create call)', async () => {
    renderHelper({ installedAgents: [] });
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.trigger/ }));
    expect(screen.getByRole('alert').textContent).toContain('mcp.custom.helper.noAgents');
    expect(createMock).not.toHaveBeenCalled();
  });
});

describe('CustomApiAiHelper — send message', () => {
  it('guards empty input (no stream fired)', async () => {
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '   ' } });
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(streamMock).not.toHaveBeenCalled();
  });

  it('sends the typed text with a form-context block prepended', async () => {
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'wire up the sessions endpoint' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(streamMock).toHaveBeenCalledTimes(1));
    const [discId, req] = streamMock.mock.calls[0];
    expect(discId).toBe('disc-c');
    expect(req.content).toContain('wire up the sessions endpoint');
    // The form snapshot (name) is part of the prepended context block.
    expect(req.content).toContain('MyAPI');
    expect(screen.getByText('wire up the sessions endpoint')).toBeTruthy();
    expect(textarea.value).toBe('');
  });

  it('streams chunks and surfaces a KRONN:APPLY suggestion card', async () => {
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk('Here is the spec.\n');
      onChunk('KRONN:APPLY\n```json\n{ "name": "Stripe API", "base_url": "https://api.stripe.com" }\n```');
      onDone();
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(screen.getByText(/Here is the spec/)).toBeTruthy());
    expect(screen.getByText(/mcp.custom.helper.suggestion/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /mcp.custom.helper.apply$/ })).toBeTruthy();
  });

  it('Apply forwards the mapped Partial<CustomApiPayload> and disables the button', async () => {
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk('KRONN:APPLY\n```json\n{ "name": "Stripe API", "base_url": "https://api.stripe.com" }\n```');
      onDone();
      return Promise.resolve();
    });
    const { onApply } = await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    const applyBtn = await screen.findByRole('button', { name: /mcp.custom.helper.apply$/ });
    fireEvent.click(applyBtn);
    expect(onApply).toHaveBeenCalledTimes(1);
    expect(onApply.mock.calls[0][0]).toEqual({
      name: 'Stripe API',
      base_url: 'https://api.stripe.com',
    });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /mcp.custom.helper.applied/ })).toBeTruthy(),
    );
  });

  it('surfaces the onError stream branch as a visible error', async () => {
    streamMock.mockImplementation((_id, _req, _onChunk, _onDone, onError) => {
      onError('stream exploded');
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(screen.getByText('stream exploded')).toBeTruthy());
  });
});

describe('CustomApiAiHelper — stop while streaming', () => {
  it('renders a stop button mid-stream and calls discussions.stop', async () => {
    streamMock.mockImplementation((_id, _req, onChunk) => {
      onChunk('thinking…');
      return new Promise(() => {});
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    const stopBtn = await screen.findByRole('button', { name: /mcp.custom.helper.stop/ });
    fireEvent.click(stopBtn);
    await waitFor(() => expect(stopMock).toHaveBeenCalledWith('disc-c'));
  });
});

describe('CustomApiAiHelper — minimize / restore / close', () => {
  it('minimizes to a restore pill and restores the bubble', async () => {
    await openChat();
    expect(screen.getByRole('dialog')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.minimize/ }));
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.restore/ }));
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it('close tears down the discussion (delete) and returns to trigger-only phase', async () => {
    await openChat();
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.close/ }));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith('disc-c'));
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('CustomApiAiHelper — agent switch', () => {
  it('switching agents kills the old disc + creates a new one with the new agent', async () => {
    await openChat({ installedAgents: ['ClaudeCode', 'Codex', 'GeminiCli'] });
    const headerTrigger = screen.getAllByRole('button').find(
      btn => btn.getAttribute('aria-haspopup') === 'listbox',
    )!;
    fireEvent.click(headerTrigger);
    expect(screen.getByRole('listbox')).toBeTruthy();
    createMock.mockClear();
    fireEvent.click(screen.getByRole('option', { name: /Gemini CLI/ }));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith('disc-c'));
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock.mock.calls[0][0].agent).toBe('GeminiCli');
  });
});
