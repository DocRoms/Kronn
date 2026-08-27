/**
 * KT-453 — "Talk about it in the discussion" from the Git diff viewer
 * prefills the composer via `window.dispatchEvent(new CustomEvent(
 * 'kronn:composer-prefill', ...))` (same idiom as `kronn:discussion-updated`),
 * since ChatInput's textarea is uncontrolled and GitPanel has no direct
 * reference to it. Pins:
 *  - an empty draft is SET to the prefill text
 *  - a non-empty draft is APPENDED to, never silently overwritten
 *  - the textarea is focused with the cursor at the end of the new text
 *  - an event for a different discussionId is ignored
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { ChatInput } from '../ChatInput';
import type { Discussion } from '../../types/generated';
import { discussions as discussionsApi } from '../../lib/api';
import { loadDraft } from '../../lib/chat-drafts';

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const baseDiscussion: Discussion = {
  id: 'd-1',
  title: 'Test discussion',
  project_id: null,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0, non_system_message_count: 0,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  archived: false, pinned: false,
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
  created_at: '2026-08-27T09:00:00Z',
  updated_at: '2026-08-27T09:00:00Z',
} as unknown as Discussion;

function renderChatInput(discussion: Discussion | null) {
  const t = (key: string, ...args: unknown[]) =>
    args.length ? `${key}(${args.join('|')})` : key;
  return render(
    <ChatInput
      discussion={discussion}
      agents={[]}
      sending={false}
      disabled={false}
      ttsEnabled={false}
      ttsState="idle"
      worktreeError={null}
      availableSkills={[]}
      availableDirectives={[]}
      onSend={vi.fn()}
      onStop={vi.fn()}
      onOrchestrate={vi.fn()}
      onTtsToggle={vi.fn()}
      onWorktreeErrorDismiss={vi.fn()}
      onWorktreeRetry={vi.fn()}
      isAgentRestricted={() => false}
      contextFiles={[]}
      uploadingFiles={false}
      toast={vi.fn() as never}
      t={t}
    />,
  );
}

function dispatchPrefill(discussionId: string, text: string) {
  act(() => {
    window.dispatchEvent(new CustomEvent('kronn:composer-prefill', { detail: { discussionId, text } }));
  });
}

describe('ChatInput — kronn:composer-prefill (KT-453)', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([]);
    vi.spyOn(discussionsApi, 'nativeAgentMode').mockResolvedValue({ disabled: false });
  });

  it('sets an empty draft to the prefill text and focuses the textarea', () => {
    renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(textarea.value).toBe('');

    dispatchPrefill('d-1', '```diff\n-old\n+new\n```\n\nAbout this block, I ');

    expect(textarea.value).toBe('```diff\n-old\n+new\n```\n\nAbout this block, I ');
    expect(document.activeElement).toBe(textarea);
    expect(textarea.selectionStart).toBe(textarea.value.length);
    expect(textarea.selectionEnd).toBe(textarea.value.length);
  });

  it('appends to a non-empty draft instead of overwriting it', () => {
    renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'already typing something' } });

    dispatchPrefill('d-1', 'About this block, I ');

    expect(textarea.value).toBe('already typing something\n\nAbout this block, I ');
  });

  it('ignores a prefill event addressed to a different discussion', () => {
    renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'my draft' } });

    dispatchPrefill('some-other-discussion', 'should not land here');

    expect(textarea.value).toBe('my draft');
  });

  it('ignores an empty prefill text', () => {
    renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'keep me' } });

    dispatchPrefill('d-1', '');

    expect(textarea.value).toBe('keep me');
  });

  it('persists the prefilled draft like any typed text — survives a reload even if the user never types again', () => {
    // Pre-fix: the prefill only called `updateChatInput` (DOM value + React
    // state), never `scheduleDraftSave` — the very next `loadDraft` call
    // (e.g. on remount after a reload) would come back empty, silently
    // dropping a draft the user could see on screen a moment before.
    vi.useFakeTimers();
    try {
      renderChatInput(baseDiscussion);
      dispatchPrefill('d-1', 'About this block, I ');
      act(() => { vi.advanceTimersByTime(300); }); // flush the 250ms debounce
      expect(loadDraft('d-1')?.text).toBe('About this block, I ');
    } finally {
      vi.useRealTimers();
    }
  });
});
