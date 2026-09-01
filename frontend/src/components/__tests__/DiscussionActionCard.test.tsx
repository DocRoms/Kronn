import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiscussionAction, DiscussionActionValue } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  get: vi.fn(),
  cancel: vi.fn(),
  launch: vi.fn(),
}));

vi.mock('../../lib/api', () => ({ discussionActions: mocks }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));
vi.mock('../RunStatusCard', () => ({
  RunStatusCard: ({ runId }: { runId?: string }) => <div data-testid="run-card">{runId}</div>,
}));

import { DiscussionActionCard } from '../DiscussionActionCard';

function variable(overrides: Partial<DiscussionActionValue> = {}): DiscussionActionValue {
  return {
    name: 'language',
    label: 'Language',
    placeholder: 'Choose a language',
    description: null,
    required: true,
    allow_manual_override: false,
    provenance: 'user_input',
    ...overrides,
  };
}

function action(overrides: Partial<DiscussionAction> = {}): DiscussionAction {
  return {
    id: 'action:msg-1:0',
    discussion_id: 'disc-1',
    source_message_id: 'msg-1',
    fence_index: 0,
    kind: 'quick_prompt',
    target_id: 'qp-1',
    target_name: 'Translate issue',
    project_id: 'project-1',
    state: 'proposed',
    values: [variable()],
    shared_run_id: null,
    result_discussion_id: null,
    deep_link: null,
    diagnostic: null,
    launched_at: null,
    finished_at: null,
    created_at: '2026-09-01T08:00:00Z',
    updated_at: '2026-09-01T08:00:00Z',
    ...overrides,
  };
}

describe('DiscussionActionCard', () => {
  beforeEach(() => vi.clearAllMocks());

  it('keeps launch human-gated and synchronously prevents a double click', async () => {
    let resolveLaunch!: (value: DiscussionAction) => void;
    mocks.launch.mockReturnValue(new Promise(resolve => { resolveLaunch = resolve; }));
    const onChanged = vi.fn();
    render(<DiscussionActionCard action={action()} onChanged={onChanged} onOpenDiscussion={vi.fn()} />);

    const launch = screen.getByRole('button', { name: /disc\.action\.launch/ });
    expect(launch).toBeDisabled();
    fireEvent.change(screen.getByRole('textbox', { name: 'Language' }), { target: { value: 'fr' } });
    expect(launch).toBeEnabled();
    fireEvent.click(launch);
    fireEvent.click(launch);

    expect(mocks.launch).toHaveBeenCalledTimes(1);
    expect(mocks.launch).toHaveBeenCalledWith('action:msg-1:0', { variables: { language: 'fr' } });

    resolveLaunch(action({ state: 'launching', values: [variable({ value: 'fr' })] }));
    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
  });

  it('shows agent suggestions as editable and can restore the original value', () => {
    render(
      <DiscussionActionCard
        action={action({
          values: [variable({
            provenance: 'agent_suggestion',
            value: 'fr',
            suggested_value: 'fr',
            suggested_by: '@claude-cli',
          })],
        })}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    const input = screen.getByRole('textbox', { name: 'Language' });
    expect(input).toHaveValue('fr');
    expect(screen.getByText('disc.action.suggestedBy:@claude-cli')).toBeInTheDocument();
    fireEvent.change(input, { target: { value: 'en' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.action\.restoreSuggestion/ }));
    expect(input).toHaveValue('fr');
  });

  it('keeps project environment references masked and read-only', () => {
    render(
      <DiscussionActionCard
        action={action({ values: [variable({
          provenance: 'project_env',
          source_ref: '<env.PROJECT_TOKEN>',
        })] })}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    expect(screen.getByDisplayValue('disc.action.resolvedAtLaunch')).toBeDisabled();
    expect(screen.getByText(/PROJECT_TOKEN/)).toBeInTheDocument();
  });

  it('lets an operator override an allow_manual_override environment value, and never requires it', async () => {
    mocks.launch.mockResolvedValue(action({ state: 'launching' }));
    render(
      <DiscussionActionCard
        action={action({ values: [variable({
          name: 'token',
          label: 'Token',
          provenance: 'project_env',
          source_ref: '<env.PROJECT_TOKEN>',
          allow_manual_override: true,
          required: true,
        })] })}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    const input = screen.getByRole('textbox', { name: 'Token' });
    expect(input).not.toBeDisabled();
    // An override-eligible env value is never a blocking requirement — Kronn
    // still resolves it when the launcher leaves the field blank.
    expect(screen.queryByText('Token *')).not.toBeInTheDocument();
    const launch = screen.getByRole('button', { name: /disc\.action\.launch/ });
    expect(launch).toBeEnabled();

    fireEvent.change(input, { target: { value: 'operator-override' } });
    fireEvent.click(launch);
    expect(mocks.launch).toHaveBeenCalledWith('action:msg-1:0', { variables: { token: 'operator-override' } });
    await waitFor(() => expect(screen.getByText('disc.action.state.launching')).toBeInTheDocument());
  });

  it('cancels an untouched proposal and opens the durable result discussion', async () => {
    const cancelled = action({ state: 'cancelled', finished_at: '2026-09-01T08:01:00Z' });
    mocks.cancel.mockResolvedValue(cancelled);
    const onChanged = vi.fn();
    const onOpenDiscussion = vi.fn();
    const view = render(
      <DiscussionActionCard action={action()} onChanged={onChanged} onOpenDiscussion={onOpenDiscussion} />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'common.cancel' }));
    await waitFor(() => expect(mocks.cancel).toHaveBeenCalledWith('action:msg-1:0'));

    view.rerender(
      <DiscussionActionCard
        action={action({ state: 'succeeded', result_discussion_id: 'disc-result' })}
        onChanged={onChanged}
        onOpenDiscussion={onOpenDiscussion}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /disc\.action\.openDiscussion/ }));
    expect(onOpenDiscussion).toHaveBeenCalledWith('disc-result');
  });

  it('renders the shared run deep-link card after a QA/QE/Workflow launch', () => {
    render(
      <DiscussionActionCard
        action={action({ kind: 'workflow', state: 'running', shared_run_id: 'run-42' })}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    expect(screen.getByTestId('run-card')).toHaveTextContent('run-42');
  });
});
