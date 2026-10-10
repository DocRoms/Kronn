/** KT-1109: several named agents announce a parallel launch, and a
 *  delegation instruction asks whether the first agent should orchestrate. */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { buildApiMock } from '../../test/apiMock';
import { ChatInput } from '../ChatInput';
import type { AgentDetection, AgentType, Discussion, DiscussionAgentHandoffMode } from '../../types/generated';
import { discussions as discussionsApi } from '../../lib/api';
import { saveDraft } from '../../lib/chat-drafts';

vi.mock('../../lib/api', () => buildApiMock({
  discussions: { participants: vi.fn().mockResolvedValue([]), nativeAgentMode: vi.fn().mockResolvedValue({ disabled: false }) },
}));

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const baseDiscussion = {
  id: 'd-delegation',
  title: 'Delegation',
  project_id: null,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  archived: false,
  pinned: false,
  workspace_mode: 'Direct',
  workspace_path: null,
  worktree_branch: null,
  tier: 'default',
  pin_first_message: false,
  summary_cache: null,
  summary_up_to_msg_idx: null,
  shared_id: null,
  shared_with: [],
  workflow_run_id: null,
  created_at: '2026-08-19T09:00:00Z',
  updated_at: '2026-08-19T09:00:00Z',
} as unknown as Discussion;

const installed = (agent_type: AgentType) => ({
  agent_type, installed: true, runtime_available: false, enabled: true,
}) as AgentDetection;

function handoffMode(effective: boolean): DiscussionAgentHandoffMode {
  return { global_enabled: effective, disabled: false, unlimited_override: false, effective_enabled: effective, paid_limit: null };
}

function renderInput(onSend: OnSend = vi.fn<SendFn>(), discussion: Discussion = baseDiscussion) {
  return { onSend, ...render(input(onSend, discussion)) };
}

type SendFn = ComponentProps<typeof ChatInput>['onSend'];
type OnSend = ReturnType<typeof vi.fn<SendFn>>;

function input(onSend: OnSend, discussion: Discussion) {
  return (
    <ChatInput
      discussion={discussion}
      agents={[installed('ClaudeCode'), installed('Codex'), installed('OpenCode')]}
      sending={false}
      disabled={false}
      ttsEnabled={false}
      ttsState="idle"
      worktreeError={null}
      availableSkills={[]}
      availableDirectives={[]}
      onSend={onSend}
      onStop={vi.fn()}
      onOrchestrate={vi.fn()}
      onTtsToggle={vi.fn()}
      onWorktreeErrorDismiss={vi.fn()}
      onWorktreeRetry={vi.fn()}
      isAgentRestricted={() => false}
      contextFiles={[]}
      uploadingFiles={false}
      toast={vi.fn() as never}
      t={(k: string) => k}
    />
  );
}

/** The attach stays pending until the returned resolver is called. */
function deferAttach(): () => void {
  let complete!: () => void;
  vi.spyOn(discussionsApi, 'update').mockImplementation(() => new Promise<void>(resolve => { complete = resolve; }));
  return () => complete();
}

async function chooseOrchestrate() {
  fireEvent.keyDown(type(DELEGATION), { key: 'Enter' });
  const orchestrate = screen.getByTestId('delegation-orchestrate') as HTMLButtonElement;
  await waitFor(() => expect(orchestrate.disabled).toBe(false));
  fireEvent.click(orchestrate);
  await waitFor(() => expect(discussionsApi.update).toHaveBeenCalled());
}

function type(text: string): HTMLTextAreaElement {
  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  fireEvent.change(textarea, { target: { value: text } });
  return textarea;
}

const DELEGATION = '@opencode tu peux lancer @codex et @claude, leur demander une blague et juger la meilleure';

describe('ChatInput multi-agent routing', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([]);
    vi.spyOn(discussionsApi, 'nativeAgentMode').mockResolvedValue({ disabled: false });
    vi.spyOn(discussionsApi, 'update').mockResolvedValue(undefined);
    vi.spyOn(discussionsApi, 'agentHandoffMode').mockResolvedValue(handoffMode(true));
  });

  it('announces a parallel launch while the draft names several agents', async () => {
    renderInput();
    type('@codex @claude une blague chacun');
    const notice = await screen.findByTestId('composer-parallel-notice');
    expect(notice.textContent).toContain('disc.parallelLaunchCount');
    expect(notice.textContent).toContain('@codex');
    expect(notice.textContent).toContain('@claude');
    type('@codex une blague');
    await waitFor(() => expect(screen.queryByTestId('composer-parallel-notice')).toBeNull());
  });

  it('sends a plain multi-agent message without asking', () => {
    const { onSend } = renderInput();
    const textarea = type('@codex @claude une blague chacun');
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(screen.queryByTestId('delegation-prompt')).toBeNull();
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend.mock.calls[0][1]).toHaveLength(2);
  });

  it('orchestrate attaches the delegated agents and sends to the first agent only', async () => {
    const { onSend } = renderInput();
    const textarea = type(DELEGATION);
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.getByTestId('delegation-prompt')).toBeTruthy();

    const orchestrate = screen.getByTestId('delegation-orchestrate') as HTMLButtonElement;
    await waitFor(() => expect(orchestrate.disabled).toBe(false));
    fireEvent.click(orchestrate);

    await waitFor(() => expect(onSend).toHaveBeenCalledTimes(1));
    expect(discussionsApi.update).toHaveBeenCalledWith('d-delegation', { attach_agents: ['Codex', 'ClaudeCode'] });
    const targets = onSend.mock.calls[0][1] ?? [];
    expect(targets).toHaveLength(1);
    expect(targets[0].agent_type).toBe('OpenCode');
    expect(screen.queryByTestId('delegation-prompt')).toBeNull();
  });

  it('the parallel choice launches every named agent and attaches nothing', async () => {
    const { onSend } = renderInput();
    fireEvent.keyDown(type(DELEGATION), { key: 'Enter' });
    fireEvent.click(screen.getByTestId('delegation-parallel'));
    await waitFor(() => expect(onSend).toHaveBeenCalledTimes(1));
    expect((onSend.mock.calls[0][1] ?? []).map(target => target.agent_type))
      .toEqual(['OpenCode', 'Codex', 'ClaudeCode']);
    expect(discussionsApi.update).not.toHaveBeenCalled();
  });

  it('a failed attachment sends nothing', async () => {
    vi.spyOn(discussionsApi, 'update').mockRejectedValue(new Error('boom'));
    const { onSend } = renderInput();
    fireEvent.keyDown(type(DELEGATION), { key: 'Enter' });
    const orchestrate = screen.getByTestId('delegation-orchestrate') as HTMLButtonElement;
    await waitFor(() => expect(orchestrate.disabled).toBe(false));
    fireEvent.click(orchestrate);
    await waitFor(() => expect(discussionsApi.update).toHaveBeenCalled());
    expect(onSend).not.toHaveBeenCalled();
  });

  it('offers only the parallel launch when collaboration is off', async () => {
    vi.spyOn(discussionsApi, 'agentHandoffMode').mockResolvedValue(handoffMode(false));
    renderInput();
    fireEvent.keyDown(type(DELEGATION), { key: 'Enter' });
    expect(await screen.findByTestId('delegation-handoff-off')).toBeTruthy();
    expect((screen.getByTestId('delegation-orchestrate') as HTMLButtonElement).disabled).toBe(true);
  });

  it('editing the text closes the question', () => {
    renderInput();
    fireEvent.keyDown(type(DELEGATION), { key: 'Enter' });
    expect(screen.getByTestId('delegation-prompt')).toBeTruthy();
    type(`${DELEGATION} !`);
    expect(screen.queryByTestId('delegation-prompt')).toBeNull();
  });
  it('a draft edited during the attach is neither sent nor cleared', async () => {
    const complete = deferAttach();
    const { onSend } = renderInput();
    await chooseOrchestrate();
    const textarea = type('@codex ce nouveau brouillon ne doit pas partir');
    expect(screen.queryByTestId('delegation-prompt')).toBeNull();
    await act(async () => { complete(); });
    expect(onSend).not.toHaveBeenCalled();
    expect(textarea.value).toBe('@codex ce nouveau brouillon ne doit pas partir');
  });

  it('switching discussion during the attach sends nothing', async () => {
    const complete = deferAttach();
    const { onSend, rerender } = renderInput();
    // The other room holds the same draft, so only the room binding can stop it.
    saveDraft('d-other', DELEGATION);
    await chooseOrchestrate();
    rerender(input(onSend, { ...baseDiscussion, id: 'd-other' } as Discussion));
    await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe(DELEGATION));
    await act(async () => { complete(); });
    expect(onSend).not.toHaveBeenCalled();
  });

  it('parallel cannot also run while the orchestration attach is pending', async () => {
    const complete = deferAttach();
    const { onSend } = renderInput();
    await chooseOrchestrate();
    const parallel = screen.getByTestId('delegation-parallel') as HTMLButtonElement;
    expect(parallel.disabled).toBe(true);
    fireEvent.click(parallel);
    expect(onSend).not.toHaveBeenCalled();
    await act(async () => { complete(); });
    await waitFor(() => expect(onSend).toHaveBeenCalledTimes(1));
    expect(onSend.mock.calls[0][0]).toBe(DELEGATION);
    expect(onSend.mock.calls[0][1]).toHaveLength(1);
  });

  it('cancelling during the attach sends nothing', async () => {
    const complete = deferAttach();
    const { onSend } = renderInput();
    await chooseOrchestrate();
    fireEvent.click(screen.getByLabelText('disc.delegationCancel'));
    await act(async () => { complete(); });
    expect(onSend).not.toHaveBeenCalled();
  });
});
