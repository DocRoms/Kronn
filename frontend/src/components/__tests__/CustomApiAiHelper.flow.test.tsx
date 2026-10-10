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
//   - switch agent keeps the old discussion and primes a new one
//   - KT-1111: the conversation is kept, linked, resumable, and no secret
//     typed in the form reaches it
//   - empty input is guarded (no stream fired)
//
// Conventions mirror the sibling base test (inline vi.mock of lib/api) and
// DebugSection.test.tsx (act + waitFor). NOTE: unlike the base test we use
// REAL timers — the lifecycle assertions wait on real microtasks.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor, within } from '@testing-library/react';

const { createMock, streamMock, deleteMock, stopMock, getMock, listMock, updateMock } = vi.hoisted(() => ({
  createMock: vi.fn(),
  streamMock: vi.fn(),
  deleteMock: vi.fn(),
  stopMock: vi.fn(),
  getMock: vi.fn(),
  listMock: vi.fn(),
  updateMock: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussions: {
    create: createMock,
    sendMessageStream: streamMock,
    runAgent: vi.fn(),
    delete: deleteMock,
    stop: stopMock,
    get: getMock,
  },
  assistantConversations: {
    list: listMock,
    update: updateMock,
  },
}));

import { CustomApiAiHelper } from '../CustomApiAiHelper';
import type { CustomApiAiHelperProps } from '../CustomApiAiHelper';
import type { AgentType } from '../../types/generated';
import { __setLocaleLoadersForTests, loadLocale } from '../../lib/i18n';
import realApplyMessage from '../../lib/__tests__/fixtures/kronn-apply-4b62875d.txt?raw';

const t: CustomApiAiHelperProps['t'] = (key, ...args) =>
  args.length === 0 ? key : `${key}(${args.join(',')})`;

const baseSnapshot: CustomApiAiHelperProps['formSnapshot'] = {
  name: 'MyAPI',
  base_url: 'https://x.test',
  description: '',
  docs_url: '',
  fields: [{ label: '', value: '' }],
  endpoints: [],
  default_headers: [],
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
  getMock.mockReset();
  listMock.mockReset().mockResolvedValue([]);
  updateMock.mockReset().mockResolvedValue(null);
  localStorage.clear();
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
  it('restores the draft and unlocks sending when locale preload fails', async () => {
    await openChat();
    const restore = __setLocaleLoadersForTests({
      fr: async () => ({ default: {} }),
      en: async () => { throw new Error('locale chunk unavailable'); },
      es: async () => ({ default: {} }),
      zh: async () => ({ default: {} }),
    });
    try {
      const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: 'keep this draft' } });
      await act(async () => fireEvent.keyDown(textarea, { key: 'Enter' }));

      await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('locale chunk unavailable'));
      expect(textarea.value).toBe('keep this draft');
      expect(streamMock).not.toHaveBeenCalled();
    } finally {
      restore();
      await Promise.all([loadLocale('fr'), loadLocale('en'), loadLocale('es'), loadLocale('zh')]);
    }
  });

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

  it('turns the real marker-in-its-own-fence reply (4b62875d) into an Apply card that fills the form', async () => {
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk(realApplyMessage);
      onDone();
      return Promise.resolve();
    });
    const { onApply } = await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'Insider Eureka' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    fireEvent.click(await screen.findByRole('button', { name: /mcp.custom.helper.apply$/ }));
    expect(onApply).toHaveBeenCalledTimes(1);
    expect(onApply.mock.calls[0][0]).toMatchObject({
      name: 'Insider Eureka Search',
      base_url: 'https://ineureka.api.useinsider.com',
      docs_url: 'https://academy.insiderone.com/docs/eureka-search-api',
    });
    expect(screen.queryByText(/aiHelper.apply.unreadable/)).toBeNull();
  });

  it('shows an unreadable-proposal notice with Retry and raw JSON when no block parses', async () => {
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk('Voici :\nKRONN:APPLY\n```json\n{ "name": "Broken", }\n```');
      onDone();
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await screen.findByText(/aiHelper.apply.unreadable/);
    expect(screen.queryByRole('button', { name: /mcp.custom.helper.apply$/ })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /aiHelper.apply.showRaw/ }));
    expect(screen.getByText('{ "name": "Broken", }')).toBeTruthy();

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /aiHelper.apply.retry$/ }));
    });
    await waitFor(() => expect(streamMock).toHaveBeenCalledTimes(2));
    expect(streamMock.mock.calls[1][1].content).toContain('aiHelper.apply.retryPrompt');
  });

  it('shows the card and the notice together when one block of two is broken', async () => {
    streamMock.mockImplementation((_id, _req, onChunk, onDone) => {
      onChunk('KRONN:APPLY\n```json\n{ "name": "Ok" }\n```\nKRONN:APPLY\n```json\n{ "name": "Broken", }\n```');
      onDone();
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await screen.findByRole('button', { name: /mcp.custom.helper.apply$/ });
    expect(screen.getByText(/aiHelper.apply.unreadable/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /aiHelper.apply.showRaw/ }));
    expect(screen.getByText('{ "name": "Broken", }')).toBeTruthy();
  });

  it('does not warn while a proposal is still streaming', async () => {
    streamMock.mockImplementation((_id, _req, onChunk) => {
      onChunk('KRONN:APPLY\n```json\n{ "name": "Str');
      return Promise.resolve();
    });
    await openChat();
    const textarea = screen.getByPlaceholderText(/mcp.custom.helper.inputPlaceholder/) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'help' } });
    await act(async () => {
      fireEvent.keyDown(textarea, { key: 'Enter' });
    });
    await waitFor(() => expect(streamMock).toHaveBeenCalledTimes(1));
    expect(screen.queryByText(/aiHelper.apply.unreadable/)).toBeNull();
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

  it('close keeps the discussion and returns to trigger-only phase', async () => {
    await openChat();
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.close/ }));
    expect(screen.queryByRole('dialog')).toBeNull();
    await act(async () => { await Promise.resolve(); });
    expect(deleteMock).not.toHaveBeenCalled();
  });

  it('unmounting keeps the discussion', async () => {
    const { unmount } = await openChat();
    unmount();
    expect(deleteMock).not.toHaveBeenCalled();
  });
});

describe('CustomApiAiHelper — agent switch', () => {
  it('switching agents keeps the old disc + creates a new one with the new agent', async () => {
    await openChat({ installedAgents: ['ClaudeCode', 'Codex', 'GeminiCli'] });
    const headerTrigger = screen.getAllByRole('button').find(
      btn => btn.getAttribute('aria-haspopup') === 'listbox',
    )!;
    fireEvent.click(headerTrigger);
    expect(screen.getByRole('listbox')).toBeTruthy();
    createMock.mockClear();
    fireEvent.click(screen.getByRole('option', { name: /Gemini CLI/ }));
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock.mock.calls[0][0].agent).toBe('GeminiCli');
    expect(deleteMock).not.toHaveBeenCalled();
  });
});

describe('CustomApiAiHelper — kept conversation (KT-1111)', () => {
  const SECRET = 'tok_live_S3cr3tValue42';
  const HEADER_SECRET = 'hdr-K3y-998877';
  const secretSnapshot: CustomApiAiHelperProps['formSnapshot'] = {
    ...baseSnapshot,
    fields: [{ label: 'API Token', value: SECRET }],
    default_headers: [
      { name: 'X-Api-Key', value: HEADER_SECRET },
      { name: 'Accept', value: 'application/json' },
    ],
  };

  it('creates and files the conversation in one call, with its secrets for the server', async () => {
    const onConversationStarted = vi.fn();
    await openChat({ targetId: 'srv-9', onConversationStarted, formSnapshot: secretSnapshot });
    expect(createMock).toHaveBeenCalledTimes(1);
    expect(createMock.mock.calls[0][0].assistant).toEqual({
      kind: 'custom_api',
      target_id: 'srv-9',
      target_label: 'MyAPI',
      secrets: [SECRET, HEADER_SECRET],
    });
    await waitFor(() => expect(onConversationStarted).toHaveBeenCalledWith('disc-c'));
    expect(listMock).toHaveBeenCalledWith({
      kind: 'custom_api', target_id: 'srv-9', include_unattached: true, pending_ids: [],
    });
  });

  it('never lets a secret typed in the form enter the kept conversation', async () => {
    streamMock.mockImplementation(async (_id, _req, onChunk, onDone) => { onChunk('ok'); onDone(); });
    await openChat({ formSnapshot: secretSnapshot });
    const input = screen.getByPlaceholderText('mcp.custom.helper.inputPlaceholder');
    const pasted = [
      `curl -H "Authorization: Bearer ${SECRET}" -H "X-Api-Key: ${HEADER_SECRET}"`,
      `encoded ${encodeURIComponent(SECRET)} b64 ${btoa(SECRET)}`,
    ].join('\n');
    fireEvent.change(input, { target: { value: pasted } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    await waitFor(() => expect(streamMock).toHaveBeenCalled());

    // The secrets travel only in the transient fields the server masks with.
    expect(streamMock.mock.calls[0][1].assistant_secrets).toEqual([SECRET, HEADER_SECRET]);
    const persisted = [
      createMock.mock.calls[0][0].initial_prompt as string,
      createMock.mock.calls[0][0].title as string,
      streamMock.mock.calls[0][1].content as string,
      ...updateMock.mock.calls.map(call => JSON.stringify(call)),
    ].join('\n');
    for (const leak of [SECRET, HEADER_SECRET, encodeURIComponent(SECRET), btoa(SECRET)]) {
      expect(persisted).not.toContain(leak);
    }
    // The agent still sees which header exists, and the user's question.
    expect(persisted).toContain('X-Api-Key');
    expect(persisted).toContain('curl -H');
    expect(screen.getByRole('dialog').textContent).not.toContain(SECRET);
  });

  it('records the last proposal and its application', async () => {
    streamMock.mockImplementation(async (_id, _req, onChunk, onDone) => {
      onChunk('Here:\nKRONN:APPLY\n```json\n{"name":"Svc"}\n```\n');
      onDone();
    });
    const { onApply } = await openChat();
    const input = screen.getByPlaceholderText('mcp.custom.helper.inputPlaceholder');
    fireEvent.change(input, { target: { value: 'go' } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    await waitFor(() => expect(updateMock).toHaveBeenCalledWith('disc-c', {
      last_proposal_signature: expect.any(String),
    }));
    const signature = updateMock.mock.calls[0][1].last_proposal_signature;
    fireEvent.click(screen.getByRole('button', { name: 'mcp.custom.helper.apply' }));
    expect(onApply).toHaveBeenCalledWith({ name: 'Svc' });
    await waitFor(() => expect(updateMock).toHaveBeenCalledWith('disc-c', { last_applied_signature: signature }));
  });

  it('"Resume" reopens the same conversation with its history', async () => {
    listMock.mockResolvedValue([{
      discussion_id: 'disc-old', kind: 'custom_api', target_id: null, target_step: null, plugin_id: null,
      target_label: 'MyAPI', last_proposal_signature: null, last_applied_signature: null, last_applied_at: null,
      created_at: '2026-10-08T10:00:00Z', title: 'helper', agent: 'Codex', archived: false, message_count: 3,
      updated_at: '2026-10-08T10:05:00Z',
    }]);
    getMock.mockResolvedValue({
      id: 'disc-old',
      messages: [
        { id: 'm0', role: 'User', channel: 'main', content: 'system prompt' },
        { id: 'm1', role: 'User', channel: 'main', content: 'ctx block\n\nmcp.custom.helper.sys.userQuestion\nhow to auth?' },
        { id: 'm2', role: 'System', channel: 'main', content: 'tool call' },
        { id: 'm3', role: 'Agent', channel: 'main', content: 'Use a bearer token.' },
      ],
    });
    renderHelper();
    const toggle = await screen.findByRole('button', { name: /aiHelper.conversations.title\(1\)/ });
    fireEvent.click(toggle);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /aiHelper.conversations.resume/ }));
    });
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('Use a bearer token.'));
    const dialog = screen.getByRole('dialog').textContent ?? '';
    expect(dialog).toContain('how to auth?');
    expect(dialog).not.toContain('system prompt');
    expect(dialog).not.toContain('ctx block');
    expect(getMock).toHaveBeenCalledWith('disc-old');
    expect(createMock).not.toHaveBeenCalled();
  });

  it('deletion is an explicit, confirmed action', async () => {
    listMock.mockResolvedValue([{
      discussion_id: 'disc-old', kind: 'custom_api', target_id: null, target_step: null, plugin_id: null,
      target_label: '', last_proposal_signature: 'a', last_applied_signature: null, last_applied_at: null,
      created_at: '2026-10-08T10:00:00Z', title: 'helper', agent: 'Codex', archived: false, message_count: 3,
      updated_at: '2026-10-08T10:05:00Z',
    }]);
    const confirmSpy = vi.fn(() => true);
    vi.stubGlobal('confirm', confirmSpy);
    renderHelper();
    fireEvent.click(await screen.findByRole('button', { name: /aiHelper.conversations.title/ }));
    expect(screen.getByText('aiHelper.conversations.status.pending')).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'aiHelper.conversations.delete' }));
    });
    expect(confirmSpy).toHaveBeenCalled();
    expect(deleteMock).toHaveBeenCalledWith('disc-old');
    vi.unstubAllGlobals();
  });
  it('masks a form secret echoed in a reply, as it streams', async () => {
    streamMock.mockImplementation(async (_id, _req, onChunk, onDone) => {
      onChunk(`Your token ${SECRET} looks fine`);
      onDone();
    });
    await openChat({ formSnapshot: secretSnapshot });
    const input = screen.getByPlaceholderText('mcp.custom.helper.inputPlaceholder');
    fireEvent.change(input, { target: { value: 'check' } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('looks fine'));
    expect(screen.getByRole('dialog').textContent).not.toContain(SECRET);
  });
});

describe('CustomApiAiHelper — resume races (KT-1111)', () => {
  const row = (id: string) => ({
    discussion_id: id, kind: 'custom_api' as const, target_id: null, target_step: null, plugin_id: null,
    target_label: id, last_proposal_signature: null, last_applied_signature: null, last_applied_at: null,
    created_at: '2026-10-08T10:00:00Z', title: id, agent: 'Codex' as const, archived: false, message_count: 2,
    updated_at: '2026-10-08T10:05:00Z',
  });
  const transcript = (reply: string) => ({
    messages: [
      { id: 'm0', role: 'User', channel: 'main', content: 'system prompt' },
      { id: 'm1', role: 'Agent', channel: 'main', content: reply },
    ],
  });
  function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>(r => { resolve = r; });
    return { promise, resolve };
  }

  async function resumeRow(id: string) {
    fireEvent.click(await screen.findByRole('button', { name: /aiHelper.conversations.title/ }));
    await act(async () => {
      fireEvent.click(within(screen.getByText(id).closest('li')!).getByRole('button', { name: /aiHelper.conversations.resume/ }));
    });
  }

  it('cannot send while the history loads, and keeps what is sent afterwards', async () => {
    listMock.mockResolvedValue([row('disc-a')]);
    const slow = deferred<ReturnType<typeof transcript>>();
    getMock.mockReturnValue(slow.promise);
    streamMock.mockImplementation(async (_id, _req, onChunk, onDone) => { onChunk('fresh reply'); onDone(); });
    renderHelper();
    await resumeRow('disc-a');
    const input = screen.getByPlaceholderText('mcp.custom.helper.inputPlaceholder') as HTMLTextAreaElement;
    expect(input.disabled).toBe(true);
    fireEvent.change(input, { target: { value: 'early' } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    expect(streamMock).not.toHaveBeenCalled();

    await act(async () => { slow.resolve(transcript('old reply')); });
    expect(input.disabled).toBe(false);
    fireEvent.change(input, { target: { value: 'after load' } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('fresh reply'));
    const text = screen.getByRole('dialog').textContent ?? '';
    expect(text).toContain('old reply');
    expect(text).toContain('after load');
  });

  it('drops a history that arrives after another conversation was opened', async () => {
    listMock.mockResolvedValue([row('disc-a'), row('disc-b')]);
    const slowA = deferred<ReturnType<typeof transcript>>();
    getMock.mockImplementation((id: string) => (id === 'disc-a' ? slowA.promise : Promise.resolve(transcript('reply of B'))));
    renderHelper();
    await resumeRow('disc-a');
    await resumeRow('disc-b');
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('reply of B'));
    await act(async () => { slowA.resolve(transcript('reply of A')); });
    expect(screen.getByRole('dialog').textContent).not.toContain('reply of A');
  });

  it('drops a history that arrives after the assistant was closed', async () => {
    listMock.mockResolvedValue([row('disc-a')]);
    const slow = deferred<ReturnType<typeof transcript>>();
    getMock.mockReturnValue(slow.promise);
    renderHelper();
    await resumeRow('disc-a');
    fireEvent.click(screen.getByRole('button', { name: /mcp.custom.helper.close/ }));
    await act(async () => { slow.resolve(transcript('stale')); });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.body.textContent).not.toContain('stale');
  });
});

describe('CustomApiAiHelper — unattached conversations (KT-1111)', () => {
  const orphan = {
    discussion_id: 'disc-orphan', kind: 'custom_api' as const, target_id: null, target_step: null, plugin_id: null,
    target_label: 'MyAPI', last_proposal_signature: null, last_applied_signature: null, last_applied_at: null,
    created_at: '2026-10-08T10:00:00Z', title: 'helper', agent: 'Codex' as const, archived: false, message_count: 2,
    updated_at: '2026-10-08T10:05:00Z',
  };

  async function resumeOrphan() {
    fireEvent.click(await screen.findByRole('button', { name: /aiHelper.conversations.title/ }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /aiHelper.conversations.resume/ }));
    });
  }

  it('reopening the created plugin offers the conversation whose attach failed, and resuming attaches it', async () => {
    listMock.mockResolvedValue([orphan]);
    getMock.mockResolvedValue({ messages: [] });
    renderHelper({ targetId: 'custom-1' });
    await resumeOrphan();
    expect(listMock).toHaveBeenCalledWith({
      kind: 'custom_api', target_id: 'custom-1', include_unattached: true, pending_ids: [],
    });
    await waitFor(() => expect(updateMock).toHaveBeenCalledWith('disc-orphan', { target_id: 'custom-1' }));
    expect(createMock).not.toHaveBeenCalled();
  });

  it('resuming a draft in the new-plugin form tracks it for the save', async () => {
    listMock.mockResolvedValue([orphan]);
    getMock.mockResolvedValue({ messages: [] });
    const onConversationStarted = vi.fn();
    renderHelper({ onConversationStarted });
    await resumeOrphan();
    await waitFor(() => expect(onConversationStarted).toHaveBeenCalledWith('disc-orphan'));
    expect(updateMock).not.toHaveBeenCalled();
  });
});

// Codex review R3 — a draft started before the plugin had its final name.
describe('Codex review — draft recovery after a name change', () => {
  it.each(['', 'InitialAPI', 'FinalAPI'])('recovers a conversation started with label %j after create succeeds and attachment fails', async (initialName) => {
    await openChat({ formSnapshot: { ...baseSnapshot, name: initialName } });
    const original = createMock.mock.calls[0][0].assistant;
    const { AssistantDraftStore } = await import('../assistantConversation');
    const drafts = new AssistantDraftStore();
    drafts.track('disc-c');
    updateMock.mockRejectedValueOnce(new Error('attachment temporarily unavailable'));
    await expect(drafts.attach('custom-created', 'FinalAPI')).rejects.toThrow();
    drafts.clear(); // resetAddMcp, after successful create.
    cleanup();
    const orphan = {
      discussion_id: 'disc-c', kind: 'custom_api' as const, target_id: null, target_step: null, plugin_id: null,
      target_label: original.target_label, last_proposal_signature: null, last_applied_signature: null, last_applied_at: null,
      created_at: '2026-10-09T10:00:00Z', title: 'helper', agent: 'Codex' as const, archived: false, message_count: 2,
      updated_at: '2026-10-09T10:05:00Z',
    };
    // Same orphan-selection predicate as db/assistant_conversations.rs.
    listMock.mockImplementation(async filter =>
      filter.include_unattached && (!filter.unattached_label || filter.unattached_label === orphan.target_label) ? [orphan] : []);
    renderHelper({ targetId: 'custom-created', formSnapshot: { ...baseSnapshot, name: 'FinalAPI' } });
    fireEvent.click(await screen.findByRole('button', { name: /aiHelper.conversations.title/ }));
    await waitFor(() => expect(listMock).toHaveBeenLastCalledWith(expect.objectContaining({ target_id: 'custom-created' })));
    expect(await screen.findByRole('button', { name: /aiHelper.conversations.resume/ })).toBeTruthy();
    // Selected by the id this browser owes the plugin, never by its label.
    expect(listMock).toHaveBeenLastCalledWith(expect.objectContaining({ pending_ids: ['disc-c'] }));
    expect(listMock.mock.lastCall?.[0]).not.toHaveProperty('unattached_label');

    getMock.mockResolvedValue({ messages: [] });
    updateMock.mockReset().mockResolvedValue(null);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /aiHelper.conversations.resume/ }));
    });
    await waitFor(() => expect(updateMock).toHaveBeenCalledWith('disc-c', { target_id: 'custom-created' }));
    const { pendingFor } = await import('../assistantConversation');
    await waitFor(() => expect(pendingFor('custom-created')).toEqual([]));
    expect(createMock).toHaveBeenCalledTimes(1);
  });
});
