import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, cleanup } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  discussions: {
    invitePeer: vi.fn(),
  },
}));

import { DiscInviteButton } from '../DiscInviteButton';
import { discussions as discussionsApi } from '../../lib/api';

const toast = vi.fn();
const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}(${args.join(',')})` : key;

const writeText = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  writeText.mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
});

const INVITE = {
  token: 'kr-join-abc',
  disc_id: 'd-4',
  expires_at: '2026-05-21T10:00:00Z',
  ttl_seconds: 600,
  instruction_text: 'disc_join({token: "kr-join-abc"})\nlis le plan avec plan_get\nreste en écoute',
  instruction_text_minimal: 'disc_join({token: "kr-join-abc"})',
};

async function openInvite() {
  (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue(INVITE);
  await act(async () => {
    render(<DiscInviteButton discId="d-4" toast={toast} t={t} />);
    await Promise.resolve();
  });
  await act(async () => {
    fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
    for (let i = 0; i < 4; i++) await Promise.resolve();
  });
}

const q = (id: string) => document.querySelector(`[data-testid="${id}"]`) as HTMLElement;
const shown = () => q('disc-invite-instruction').textContent;

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('DiscInviteButton', () => {
  /// KT-1146 — the token alone is what is usually needed: the click copies it
  /// and nothing else, and the modal says so.
  it('copies exactly the token on click and announces it', async () => {
    await openInvite();
    expect(discussionsApi.invitePeer).toHaveBeenCalledWith('d-4');
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith('kr-join-abc');
    expect(q('disc-invite-notice').textContent).toBe('disc.inviteTokenCopied');
    expect(q('disc-invite-notice').getAttribute('role')).toBe('status');
    expect(shown()).toBe('kr-join-abc');
  });

  it('offers three tabs, token first and selected, and no checkbox', async () => {
    await openInvite();
    const tabs = Array.from(document.querySelectorAll('[role="tablist"] [role="tab"]'));
    expect(tabs.map(el => el.textContent)).toEqual([
      'disc.inviteTabToken', 'disc.inviteTabSimple', 'disc.inviteTabDetailed',
    ]);
    expect(q('disc-invite-tab-token').getAttribute('aria-selected')).toBe('true');
    expect(q('disc-invite-tab-simple').getAttribute('aria-selected')).toBe('false');
    expect(document.querySelector('input[type="checkbox"]')).toBeNull();
    expect(document.querySelector('[role="tabpanel"]')).not.toBeNull();
  });

  it('copies each tab its own text', async () => {
    await openInvite();
    writeText.mockClear();

    await act(async () => { fireEvent.click(q('disc-invite-copy-token')); });
    expect(writeText).toHaveBeenLastCalledWith('kr-join-abc');

    await act(async () => { fireEvent.click(q('disc-invite-tab-simple')); });
    expect(shown()).toBe(INVITE.instruction_text_minimal);
    await act(async () => { fireEvent.click(q('disc-invite-copy-simple')); });
    expect(writeText).toHaveBeenLastCalledWith(INVITE.instruction_text_minimal);

    await act(async () => { fireEvent.click(q('disc-invite-tab-detailed')); });
    expect(shown()).toBe(INVITE.instruction_text);
    await act(async () => { fireEvent.click(q('disc-invite-copy-detailed')); });
    expect(writeText).toHaveBeenLastCalledWith(INVITE.instruction_text);
    expect(writeText).toHaveBeenCalledTimes(3);
  });

  it('moves between tabs with the arrow keys, Home and End', async () => {
    await openInvite();
    fireEvent.keyDown(q('disc-invite-tab-token'), { key: 'ArrowRight' });
    expect(q('disc-invite-tab-simple').getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(q('disc-invite-tab-simple'));
    fireEvent.keyDown(q('disc-invite-tab-simple'), { key: 'End' });
    expect(q('disc-invite-tab-detailed').getAttribute('aria-selected')).toBe('true');
    fireEvent.keyDown(q('disc-invite-tab-detailed'), { key: 'ArrowRight' });
    expect(q('disc-invite-tab-token').getAttribute('aria-selected')).toBe('true');
    fireEvent.keyDown(q('disc-invite-tab-token'), { key: 'ArrowLeft' });
    expect(q('disc-invite-tab-detailed').getAttribute('aria-selected')).toBe('true');
    fireEvent.keyDown(q('disc-invite-tab-detailed'), { key: 'Home' });
    expect(q('disc-invite-tab-token').getAttribute('tabindex')).toBe('0');
    expect(q('disc-invite-tab-simple').getAttribute('tabindex')).toBe('-1');
  });

  /// A refused clipboard must not read as a copy: the fallback says so and the
  /// token stays visible and selected for a manual copy.
  it('shows the manual fallback and selects the token when the clipboard is refused', async () => {
    writeText.mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
    await openInvite();
    const notice = q('disc-invite-notice');
    expect(notice.textContent).toBe('disc.inviteCopyManual');
    expect(notice.getAttribute('role')).toBe('alert');
    expect(document.body.textContent).not.toContain('disc.inviteTokenCopied');
    expect(shown()).toBe('kr-join-abc');
    expect(window.getSelection()?.toString()).toBe('kr-join-abc');
  });

  it('falls back too when the clipboard API is missing', async () => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
    await openInvite();
    expect(q('disc-invite-notice').textContent).toBe('disc.inviteCopyManual');
  });

  /// WebKit (the Tauri desktop webview) refuses a write started after an
  /// await, so the promise-backed item must be handed over inside the click.
  describe('promise-backed ClipboardItem (WebKit)', () => {
    const items: Array<Record<string, Promise<Blob>>> = [];
    const write = vi.fn();
    beforeEach(() => {
      items.length = 0;
      vi.stubGlobal('isSecureContext', true);
      vi.stubGlobal('ClipboardItem', vi.fn(function (data: Record<string, Promise<Blob>>) { items.push(data); }));
      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText, write } });
    });

    async function clickWithPendingMint() {
      let resolveMint!: (v: typeof INVITE) => void;
      (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(r => { resolveMint = r; }));
      await act(async () => { render(<DiscInviteButton discId="d-4" toast={toast} t={t} />); });
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      return async () => {
        await act(async () => {
          resolveMint(INVITE);
          for (let i = 0; i < 10; i++) await Promise.resolve();
        });
      };
    }

    it('starts the write synchronously in the click, before the mint resolves', async () => {
      write.mockImplementation(async (list: unknown[]) => { await items[0]['text/plain']; return list; });
      const finish = await clickWithPendingMint();
      expect(write).toHaveBeenCalledTimes(1);
      expect(items).toHaveLength(1);
      expect(Object.keys(items[0])).toEqual(['text/plain']);
      await finish();
      expect(await (await items[0]['text/plain']).text()).toBe('kr-join-abc');
      expect(writeText).not.toHaveBeenCalled();
      expect(q('disc-invite-notice').textContent).toBe('disc.inviteTokenCopied');
    });

    it('falls back to writeText when the item write is refused', async () => {
      write.mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
      const finish = await clickWithPendingMint();
      await finish();
      expect(writeText).toHaveBeenCalledWith('kr-join-abc');
      expect(q('disc-invite-notice').textContent).toBe('disc.inviteTokenCopied');
    });

    it('shows the manual fallback when both writes are refused', async () => {
      write.mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
      writeText.mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
      const finish = await clickWithPendingMint();
      await finish();
      expect(q('disc-invite-notice').textContent).toBe('disc.inviteCopyManual');
      expect(window.getSelection()?.toString()).toBe('kr-join-abc');
    });
  });

  it('toasts an error when the invite-peer call fails', async () => {
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('boom')
    );
    await act(async () => {
      render(<DiscInviteButton discId="d-err" toast={toast} t={t} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
    });
    // toast(_key, 'error') called with the failure key.
    const errToast = toast.mock.calls.find(c => c[1] === 'error');
    expect(errToast).toBeDefined();
    expect(errToast![0]).toContain('disc.inviteFailed');
  });

  /// The chip strip lives in another component now, so the only way it learns
  /// about a peer invited here is this callback. Without it the new peer waits
  /// out the poll interval.
  it('tells the participants row to refetch after a successful invite', async () => {
    const onInvited = vi.fn();
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue({
      token: 'kr-join-ok',
      disc_id: 'd-6',
      expires_at: '2026-05-21T10:00:00Z',
      ttl_seconds: 600,
      instruction_text: 'disc_join({token: "kr-join-ok"})',
      instruction_text_minimal: 'disc_join({token: "kr-join-ok"})',
    });
    await act(async () => {
      render(<DiscInviteButton discId="d-6" toast={toast} t={t} onInvited={onInvited} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onInvited).toHaveBeenCalledTimes(1);
  });

  it('does not call back when the invite fails', async () => {
    const onInvited = vi.fn();
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('nope'));
    await act(async () => {
      render(<DiscInviteButton discId="d-7" toast={toast} t={t} onInvited={onInvited} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
    });
    expect(onInvited).not.toHaveBeenCalled();
  });
});
