// KT-982 — talking about one sheet reaches the chosen agents through the
// ordinary discussion machinery.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
    locale: 'fr',
  }),
}));

import { TdTalkPanel } from '../TdTalkPanel';
import { discussions as discussionsApi, projects as projectsApi } from '../../lib/api';
import { tdItem } from '../../test/tdFixtures';

const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;
const CHOICES = [{ agent: 'ClaudeCode' as const }, { agent: 'Codex' as const }];

function renderPanel(onTalkChanged = vi.fn()) {
  render(
    <TdTalkPanel
      projectId="p1"
      item={tdItem('TD-20261001-a', 'High')}
      choices={CHOICES}
      unusable={[{ connectionId: 'c-off', label: 'Local', reason: 'no_model' }]}
      installedAgentTypes={['ClaudeCode', 'Codex']}
      targets={[{ agent: 'ClaudeCode', tier: 'default' }, { agent: 'Codex', tier: 'reasoning' }]}
      onTargetsChange={vi.fn()}
      onTalkChanged={onTalkChanged}
      onOpenDiscussion={vi.fn()}
    />,
  );
}

beforeEach(() => vi.clearAllMocks());

describe('TdTalkPanel', () => {
  it('shows the validation mentions and names the agents in the input', async () => {
    mocked(projectsApi.tdTalk).mockResolvedValueOnce({
      validation_discussion_id: 'dv', discussion_id: null,
      messages: [{
        id: 'm1', discussion_id: 'dv', source: 'validation', role: 'Agent', agent_type: 'ClaudeCode',
        author: null, content: 'Sur TD-20261001-a : vrai problème.', excerpt: true, timestamp: '2026-10-01T10:00:00Z',
      }],
    });
    renderPanel();
    expect(await screen.findByText('Sur TD-20261001-a : vrai problème.')).toBeInTheDocument();
    expect(screen.getByText('techDebt.talk.fromValidation')).toBeInTheDocument();
    expect(screen.getByPlaceholderText(/techDebt\.talk\.placeholder .*Claude.*Codex/)).toBeInTheDocument();
    expect(screen.getByTestId('td-talk-chip-unusable-c-off')).toBeInTheDocument();
  });

  it('creates the sheet discussion with every target then runs it', async () => {
    mocked(projectsApi.startTdTalk).mockResolvedValueOnce({ discussion_id: 'dt', created: true });
    mocked(discussionsApi.runAgent).mockImplementationOnce(async (_id, _chunk, done) => { done(); });
    const onTalkChanged = vi.fn();
    renderPanel(onTalkChanged);
    fireEvent.change(screen.getByLabelText('techDebt.talk.title'), { target: { value: 'Vrai problème ?' } });
    fireEvent.click(screen.getByTestId('td-talk-send'));
    await waitFor(() => expect(projectsApi.startTdTalk).toHaveBeenCalledWith('p1', 'TD-20261001-a', {
      message: 'Vrai problème ?',
      targets: [
        { kind: 'agent', agent_type: 'ClaudeCode', tier: 'default' },
        { kind: 'agent', agent_type: 'Codex', tier: 'reasoning' },
      ],
    }));
    await waitFor(() => expect(discussionsApi.runAgent).toHaveBeenCalledWith('dt', expect.any(Function), expect.any(Function), expect.any(Function)));
    await waitFor(() => expect(onTalkChanged).toHaveBeenCalled());
    expect(discussionsApi.sendMessageStream).not.toHaveBeenCalled();
  });

  it('sends to the existing sheet discussion with the targets', async () => {
    mocked(projectsApi.startTdTalk).mockResolvedValueOnce({ discussion_id: 'dt', created: false });
    renderPanel();
    fireEvent.change(screen.getByLabelText('techDebt.talk.title'), { target: { value: 'Et ensuite ?' } });
    fireEvent.click(screen.getByTestId('td-talk-send'));
    await waitFor(() => expect(discussionsApi.sendMessageStream).toHaveBeenCalledWith(
      'dt',
      { content: 'Et ensuite ?', targets: expect.arrayContaining([expect.objectContaining({ agent_type: 'Codex' })]) },
      expect.any(Function), expect.any(Function), expect.any(Function),
    ));
    expect(discussionsApi.runAgent).not.toHaveBeenCalled();
  });
});
