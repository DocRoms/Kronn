/**
 * Regression guard for the "draft lost on tab switch" bug reported on
 * 2026-04-15. ChatInput's textarea is non-controlled (DOM-ref based for
 * perf), so a remount — which happens every time the user leaves the
 * Discussions page — used to wipe the in-flight message. The fix
 * persists drafts per-discussion in localStorage; these tests prove
 * the round-trip survives an actual unmount/remount cycle.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { ChatInput } from '../ChatInput';
import type { Discussion } from '../../types/generated';
import { loadDraft, saveDraft, CHAT_DRAFT_CONFIG } from '../../lib/chat-drafts';

// ─── Mocks: heavy dependencies ChatInput transitively loads ─────────────

// The STT worker import pulls a Web Worker URL that vitest cannot resolve;
// swap it out with a no-op so ChatInput mounts in JSDOM.
vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

// ─── Test helpers ──────────────────────────────────────────────────────

const baseDiscussion: Discussion = {
  id: 'd-1',
  title: 'Test discussion',
  project_id: null,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
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
  created_at: '2026-04-15T09:00:00Z',
  updated_at: '2026-04-15T09:00:00Z',
} as unknown as Discussion;

function renderChatInput(discussion: Discussion | null, sending = false) {
  const t = (key: string, ...args: unknown[]) =>
    args.length ? `${key}(${args.join('|')})` : key;
  return render(
    <ChatInput
      discussion={discussion}
      agents={[]}
      sending={sending}
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

describe('ChatInput draft persistence', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-15T09:00:00Z'));
  });

  it('saves typed text to localStorage (debounced) under kronn:draft:<disc_id>', () => {
    renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox');
    fireEvent.change(textarea, { target: { value: 'hello in-flight' } });
    // Debounced at 250 ms — advance timers to flush.
    act(() => { vi.advanceTimersByTime(300); });

    const saved = loadDraft('d-1');
    expect(saved).not.toBeNull();
    expect(saved!.text).toBe('hello in-flight');
    // Storage key uses the kronn:draft: prefix.
    expect(localStorage.getItem(CHAT_DRAFT_CONFIG.KEY_PREFIX + 'd-1')).not.toBeNull();
  });

  it('restores the draft into the textarea on remount (the reported bug)', () => {
    // User typed + the page was unmounted before the debounce flushed — mimic
    // by seeding storage directly.
    saveDraft('d-1', 'resume this message');

    const { unmount } = renderChatInput(baseDiscussion);
    const first = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(first.value).toBe('resume this message');

    // Simulate leaving and coming back to the tab — fresh render.
    unmount();
    renderChatInput(baseDiscussion);
    const second = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(second.value).toBe('resume this message');

    // The "restored draft" hint is visible with a relative-time marker.
    expect(screen.getByRole('status')).toHaveTextContent('disc.draftRestored');
  });

  it('clears the stored draft after a successful send', () => {
    saveDraft('d-1', 'about to send');
    const onSend = vi.fn();
    render(
      <ChatInput
        discussion={baseDiscussion}
        agents={[]}
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
      />,
    );
    const textarea = screen.getByRole('textbox');
    // Enter → send (no shift).
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: false });

    expect(onSend).toHaveBeenCalledWith('about to send', undefined);
    expect(loadDraft('d-1')).toBeNull();
  });

  it('keeps drafts isolated between discussions', () => {
    saveDraft('d-1', 'text for one');
    saveDraft('d-2', 'text for two');

    const { unmount } = renderChatInput(baseDiscussion);
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('text for one');
    unmount();

    const otherDisc = { ...baseDiscussion, id: 'd-2' } as Discussion;
    renderChatInput(otherDisc);
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('text for two');
  });

  it('editing the restored draft hides the "restored" hint', () => {
    saveDraft('d-1', 'saved text');
    renderChatInput(baseDiscussion);

    expect(screen.getByRole('status')).toBeInTheDocument();

    const textarea = screen.getByRole('textbox');
    fireEvent.change(textarea, { target: { value: 'saved text + more' } });

    // Hint gone once the user starts editing — it was a passive reminder,
    // not a persistent banner.
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('rerender to a different discussion CLEARS the textarea (regression: 2026-04-15)', () => {
    // Reported bug: "le même message 'pas encore envoyé' apparaît dans toutes
    // les discussions que j'ouvre". Root cause candidate: ChatInput is NOT
    // remounted when the user switches discussions in DiscussionsPage — just
    // re-rendered with a new `discussion` prop. The previous test case used
    // unmount+remount which masked the race; this one exercises the actual
    // live-rerender path and asserts the textarea DOM value, not just
    // localStorage.
    const { rerender } = renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;

    fireEvent.change(textarea, { target: { value: 'typed in d-1' } });
    expect(textarea.value).toBe('typed in d-1');

    // Switch to d-2 with NO saved draft — the textarea must reset to empty.
    const otherDisc = { ...baseDiscussion, id: 'd-2' } as Discussion;
    rerender(
      <ChatInput
        discussion={otherDisc}
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
        t={(k: string) => k}
      />,
    );

    const sameTextarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(sameTextarea.value).toBe('');
    // And d-1's draft must survive in storage (separate concern: we're not
    // dropping the user's in-flight message, we're only clearing the UI).
    expect(loadDraft('d-1')!.text).toBe('typed in d-1');
  });

  it('rerender to a discussion WITH an existing draft rehydrates the textarea', () => {
    saveDraft('d-2', 'resume me in d-2');
    const { rerender } = renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: 'in d-1' } });

    const otherDisc = { ...baseDiscussion, id: 'd-2' } as Discussion;
    rerender(
      <ChatInput
        discussion={otherDisc}
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
        t={(k: string) => k}
      />,
    );

    const after = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(after.value).toBe('resume me in d-2');
  });

  it('switching discussions flushes the previous draft without waiting for debounce', () => {
    const { rerender } = renderChatInput(baseDiscussion);
    const textarea = screen.getByRole('textbox');
    fireEvent.change(textarea, { target: { value: 'typed in d-1' } });
    // Don't advance timers — simulate an immediate tab-switch.

    const otherDisc = { ...baseDiscussion, id: 'd-2' } as Discussion;
    rerender(
      <ChatInput
        discussion={otherDisc}
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
        t={(k: string) => k}
      />,
    );

    // The d-1 draft should now be in storage even though the debounce
    // timer never fired — the pre-switch flush took care of it.
    expect(loadDraft('d-1')!.text).toBe('typed in d-1');
  });
});
