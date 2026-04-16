/**
 * QP Chain Phase 1 UI coverage — the picker/badge that lets the user queue
 * a Quick Prompt while the agent is still streaming. The auto-fire on
 * `sending: true → false` lives in DiscussionsPage and is not exercised
 * here; this file guards the ChatInput surface area only.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ChatInput } from '../ChatInput';
import type { Discussion, QuickPrompt } from '../../types/generated';

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const baseDiscussion: Discussion = {
  id: 'd-qp',
  title: 'QP chain',
  project_id: null,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  archived: false,
  pinned: false,
  workspace_mode: 'Direct',
  workspace_path: null,
  worktree_branch: null,
  tier: 'Default',
  pin_first_message: false,
  summary_cache: null,
  summary_up_to_msg_idx: null,
  shared_id: null,
  shared_with: [],
  workflow_run_id: null,
  created_at: '2026-04-17T09:00:00Z',
  updated_at: '2026-04-17T09:00:00Z',
} as unknown as Discussion;

function makeQP(overrides: Partial<QuickPrompt> = {}): QuickPrompt {
  return {
    id: 'qp-review',
    name: 'Review code',
    icon: '🔍',
    description: 'Agent review pass',
    prompt_template: 'Review the code you just wrote.',
    variables: [],
    category: null,
    ...overrides,
  } as unknown as QuickPrompt;
}

function baseProps(extra: Partial<Parameters<typeof ChatInput>[0]> = {}) {
  const t = (key: string, ...args: unknown[]) =>
    args.length ? `${key}(${args.join('|')})` : key;
  return {
    discussion: baseDiscussion,
    agents: [],
    sending: true,
    disabled: false,
    ttsEnabled: false,
    ttsState: 'idle' as const,
    worktreeError: null,
    availableSkills: [],
    availableDirectives: [],
    onSend: vi.fn(),
    onStop: vi.fn(),
    onOrchestrate: vi.fn(),
    onTtsToggle: vi.fn(),
    onWorktreeErrorDismiss: vi.fn(),
    onWorktreeRetry: vi.fn(),
    isAgentRestricted: () => false,
    contextFiles: [],
    uploadingFiles: false,
    toast: vi.fn() as never,
    t,
    ...extra,
  };
}

describe('ChatInput — QP chain picker', () => {
  it('shows the chain-QP button while sending when chainable QPs are provided', () => {
    const onQueue = vi.fn();
    render(<ChatInput {...baseProps({ chainableQPs: [makeQP()], onQueueQP: onQueue })} />);
    // The picker button uses the `disc.chainQP` aria-label.
    expect(screen.getByLabelText('disc.chainQP')).toBeInTheDocument();
  });

  it('hides the chain button when there are no chainable QPs', () => {
    render(<ChatInput {...baseProps({ chainableQPs: [], onQueueQP: vi.fn() })} />);
    expect(screen.queryByLabelText('disc.chainQP')).toBeNull();
  });

  it('does NOT render the chain button while the agent is idle (sending=false)', () => {
    // The picker is specifically for the "while streaming" state so the user
    // can queue the next prompt before the current one finishes.
    render(<ChatInput {...baseProps({ sending: false, chainableQPs: [makeQP()], onQueueQP: vi.fn() })} />);
    expect(screen.queryByLabelText('disc.chainQP')).toBeNull();
  });

  it('clicking a QP in the picker calls onQueueQP with the full QP', () => {
    const onQueue = vi.fn();
    const qp = makeQP();
    render(<ChatInput {...baseProps({ chainableQPs: [qp], onQueueQP: onQueue })} />);

    fireEvent.click(screen.getByLabelText('disc.chainQP'));
    // The popover item shows the QP name.
    fireEvent.mouseDown(screen.getByText('Review code'));

    expect(onQueue).toHaveBeenCalledTimes(1);
    expect(onQueue).toHaveBeenCalledWith(qp);
  });

  it('renders the queued badge instead of the picker when a QP is queued', () => {
    const onCancel = vi.fn();
    render(<ChatInput {...baseProps({
      chainableQPs: [makeQP()],
      onQueueQP: vi.fn(),
      queuedQP: makeQP(),
      onCancelQueuedQP: onCancel,
    })} />);
    // Badge shows QP name and icon; picker button is hidden (only one slot).
    expect(screen.getByTitle('disc.cancelQueuedQP')).toBeInTheDocument();
    expect(screen.queryByLabelText('disc.chainQP')).toBeNull();
  });

  it('clicking the queued badge calls onCancelQueuedQP', () => {
    const onCancel = vi.fn();
    render(<ChatInput {...baseProps({
      chainableQPs: [makeQP()],
      onQueueQP: vi.fn(),
      queuedQP: makeQP(),
      onCancelQueuedQP: onCancel,
    })} />);
    fireEvent.click(screen.getByTitle('disc.cancelQueuedQP'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
