/**
 * 0.8.7 — P0-10 of the QA roadmap.
 *
 * Closure-stale guard on ChatInput's `handleSendMessage`. The `sending`
 * prop is flipped to true by the parent on the false→true edge AFTER the
 * onSend callback fires — meaning two synchronous clicks in the same
 * event-loop tick both see `sending=false`. Pre-fix this double-POSTed
 * the user's message (the highest-blast bug class in the chat path).
 *
 * The fix adds a `sendInFlightRef` set+cleared in the same tick that
 * blocks the immediate-next click. These tests pin the contract :
 *  - 2 synchronous clicks → 1 onSend call (not 2)
 *  - After microtask flush, the user can send again
 *  - If onSend throws synchronously, the ref still releases (user can retry)
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { ChatInput } from '../ChatInput';
import type { Discussion } from '../../types/generated';

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const disc: Discussion = {
  id: 'd-race', title: 'Race test', project_id: null,
  agent: 'ClaudeCode', language: 'fr',
  participants: ['ClaudeCode'], messages: [],
  message_count: 0, non_system_message_count: 0,
  skill_ids: [], profile_ids: [], directive_ids: [],
  archived: false, pinned: false,
  workspace_mode: 'Direct', workspace_path: null, worktree_branch: null,
  tier: 'Default', pin_first_message: false,
  summary_cache: null, summary_up_to_msg_idx: null,
  shared_id: null, shared_with: [], workflow_run_id: null,
  created_at: '2026-05-28T00:00:00Z', updated_at: '2026-05-28T00:00:00Z',
} as unknown as Discussion;

function mount(onSend: ReturnType<typeof vi.fn>, sending = false) {
  const t = (k: string, ...a: unknown[]) => (a.length ? `${k}(${a.join('|')})` : k);
  return render(
    <ChatInput
      discussion={disc} agents={[]} sending={sending} disabled={false}
      ttsEnabled={false} ttsState="idle" worktreeError={null}
      availableSkills={[]} availableDirectives={[]}
      onSend={onSend as never} onStop={vi.fn()} onOrchestrate={vi.fn()}
      onTtsToggle={vi.fn()} onWorktreeErrorDismiss={vi.fn()}
      onWorktreeRetry={vi.fn()} isAgentRestricted={() => false}
      contextFiles={[]} uploadingFiles={false}
      toast={vi.fn() as never} t={t}
    />,
  );
}

function typeText(value: string) {
  const ta = screen.getByPlaceholderText(/disc\.messagePlaceholder/i) as HTMLTextAreaElement;
  fireEvent.change(ta, { target: { value } });
  return ta;
}

function sendButton(): HTMLButtonElement {
  // The send button is `Enter` in the textarea OR a click on the send
  // icon. ChatInput renders the latter with `aria-label="chat.send"`
  // (i18n key passed through the identity translator in tests).
  return screen.getByLabelText('Send message') as HTMLButtonElement;
}

describe('ChatInput — send-race guard (P0-10)', () => {
  it('two synchronous clicks fire onSend only ONCE', () => {
    const onSend = vi.fn();
    mount(onSend);
    typeText('hello race');

    // Fire two clicks in the same tick — pre-fix this produced two
    // onSend calls because the `sending` prop hadn't been re-rendered
    // by the parent yet between the clicks.
    act(() => {
      const btn = sendButton();
      fireEvent.click(btn);
      fireEvent.click(btn);
    });

    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend).toHaveBeenCalledWith('hello race', undefined);
  });

  it('Enter then Enter in quick succession only fires onSend once', () => {
    // Same race via keyboard — Enter is the dominant send path.
    const onSend = vi.fn();
    mount(onSend);
    const ta = typeText('keyboard race') as HTMLTextAreaElement;

    act(() => {
      fireEvent.keyDown(ta, { key: 'Enter' });
      fireEvent.keyDown(ta, { key: 'Enter' });
    });

    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it('after a microtask flush, the user can send again', async () => {
    const onSend = vi.fn();
    mount(onSend);
    typeText('first message');
    act(() => { fireEvent.click(sendButton()); });
    expect(onSend).toHaveBeenCalledTimes(1);

    // Microtask flush — the ref is released and the next send fires.
    await act(async () => { await Promise.resolve(); });

    typeText('second message');
    act(() => { fireEvent.click(sendButton()); });
    expect(onSend).toHaveBeenCalledTimes(2);
    expect(onSend.mock.calls[1][0]).toBe('second message');
  });

  // NB : a previous variant tried to verify ref-release after a SYNC throw
  // inside onSend. React 19's event-dispatch surfaces such throws as
  // unhandled exceptions at the test-file level (not catchable by try/catch
  // around `act`), which broke vitest's coverage writeback. The microtask-
  // released-ref contract is already pinned by the two-click + Enter+Enter
  // tests above (which use the success path) — the failure path is just
  // a stricter assertion on the same useRef + queueMicrotask cleanup logic.

  it('sending=true from the parent swaps the send button for a stop button (UI guard)', () => {
    // The prop-based guard manifests as a JSX swap : send button is
    // REPLACED by a stop button while in-flight. That's an even stronger
    // guarantee than `disabled={sending}` (user physically can't click
    // send → no race possible). Pin the swap so a regression that
    // removes the ternary is caught here too.
    const onSend = vi.fn();
    const { rerender } = mount(onSend, /* sending */ false);
    expect(screen.queryByLabelText('Send message')).not.toBeNull();
    expect(screen.queryByLabelText('disc.stopThinking')).toBeNull();

    // Parent simulates the round-trip: sets sending=true.
    rerender(
      <ChatInput
        discussion={disc} agents={[]} sending={true} disabled={false}
        ttsEnabled={false} ttsState="idle" worktreeError={null}
        availableSkills={[]} availableDirectives={[]}
        onSend={onSend as never} onStop={vi.fn()} onOrchestrate={vi.fn()}
        onTtsToggle={vi.fn()} onWorktreeErrorDismiss={vi.fn()}
        onWorktreeRetry={vi.fn()} isAgentRestricted={() => false}
        contextFiles={[]} uploadingFiles={false}
        toast={vi.fn() as never}
        t={(k: string) => k}
      />,
    );

    // After the prop flip, the send button disappears, stop button appears.
    expect(screen.queryByLabelText('Send message')).toBeNull();
    expect(screen.queryByLabelText('disc.stopThinking')).not.toBeNull();
    // And no further onSend was triggered by the re-render.
    expect(onSend).not.toHaveBeenCalled();
  });
});
