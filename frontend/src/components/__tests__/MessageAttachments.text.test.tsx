import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ContextFile } from '../../types/generated';

const { preview, blob, download } = vi.hoisted(() => ({ preview: vi.fn(), blob: vi.fn(), download: vi.fn() }));
vi.mock('../../lib/api', () => ({
  discussions: { contextFileTextPreview: preview, contextFileBlob: blob },
  media: { capabilities: vi.fn() },
}));
vi.mock('../../lib/downloadBlob', () => ({ triggerDownload: download }));
import { MessageAttachments } from '../MessageAttachments';

const t = (key: string, ...args: (string | number)[]) => `${key}${args.length ? ':' + args.join(',') : ''}`;
const file = (id = 'json', overrides: Partial<ContextFile> = {}): ContextFile => ({
  id, discussion_id: 'disc', filename: `${id}.json`, mime_type: 'application/json',
  original_size: 1024, extracted_size: 0, disk_path: `/tmp/${id}`, message_id: 'message',
  ai_generation: null, created_at: '2026-09-22T00:00:00Z', ...overrides,
});

beforeEach(() => {
  vi.clearAllMocks();
  preview.mockResolvedValue({ text: '{"ok":true}', truncated: false });
  blob.mockResolvedValue(new Blob(['original']));
  globalThis.URL.createObjectURL = vi.fn(() => 'blob:preview');
  globalThis.URL.revokeObjectURL = vi.fn();
});

describe('text attachments in the shared carousel', () => {
  it('loads only on opening, renders markup as text, and downloads the original', async () => {
    const text = '<script>alert("x")</script><img src=x onerror=alert(1)>';
    preview.mockResolvedValue({ text, truncated: true });
    render(<MessageAttachments files={[file()]} discussionId="disc" t={t} />);
    expect(preview).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentText:json.json' }));
    const dialog = await screen.findByRole('dialog');
    expect(await within(dialog).findByText(text)).toBeInTheDocument();
    expect(dialog.querySelector('script, img, iframe')).toBeNull();
    expect(within(dialog).getByRole('note')).toHaveTextContent('disc.attachmentTextTruncated');
    expect(within(dialog).queryByRole('link')).toBeNull();
    fireEvent.click(within(dialog).getByRole('button', { name: 'disc.assets.downloadFor:json.json' }));
    await waitFor(() => expect(download).toHaveBeenCalledWith('json.json', expect.any(Blob)));
    expect(blob).toHaveBeenCalledWith('disc', 'json');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('copies the whole original text from the carousel in a secure context', async () => {
    class FakeItem { constructor(readonly items: Record<string, Blob | Promise<Blob>>) {} }
    const copied: string[] = [];
    const write = vi.fn(async (items: FakeItem[]) => {
      for (const item of items) copied.push(await (await item.items['text/plain']).text());
    });
    vi.stubGlobal('ClipboardItem', FakeItem);
    const restore = [
      Object.getOwnPropertyDescriptor(window, 'isSecureContext'),
      Object.getOwnPropertyDescriptor(navigator, 'clipboard'),
    ];
    Object.defineProperty(window, 'isSecureContext', { configurable: true, value: true });
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn(), write } });
    try {
      preview.mockResolvedValue({ text: 'preview only', truncated: true });
      render(<MessageAttachments files={[file()]} discussionId="disc" t={t} />);
      fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentText:json.json' }));
      const dialog = await screen.findByRole('dialog');
      fireEvent.click(within(dialog).getByRole('button', { name: 'disc.assets.copy' }));
      await waitFor(() => expect(copied).toEqual(['original']));
      expect(within(dialog).getByTestId('attachment-copy')).toHaveAccessibleName('disc.assets.copied');

      // Two clicks in the same tick must not start two downloads.
      blob.mockClear();
      const button = within(dialog).getByTestId('attachment-copy');
      act(() => { button.click(); button.click(); });
      await waitFor(() => expect(copied).toHaveLength(2));
      expect(blob).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
      for (const [target, key, descriptor] of [[window, 'isSecureContext', restore[0]], [navigator, 'clipboard', restore[1]]] as const) {
        if (descriptor) Object.defineProperty(target, key, descriptor);
        else delete (target as unknown as Record<string, unknown>)[key];
      }
    }
  });

  it('explains why copying is unavailable outside a secure context', async () => {
    render(<MessageAttachments files={[file()]} discussionId="disc" t={t} />);
    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentText:json.json' }));
    const button = within(await screen.findByRole('dialog')).getByTestId('attachment-copy');
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleName('disc.assets.copyInsecure');
  });

  it('walks between text, images and videos through the whole discussion', async () => {
    const text = file();
    const scope = [text, file('picture', { mime_type: 'image/png', filename: 'picture.png' }), file('clip', { mime_type: 'video/mp4', filename: 'clip.mp4' })];
    render(<MessageAttachments files={[text]} carouselScope={scope} discussionId="disc" t={t} />);
    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentText:json.json' }));
    expect(await screen.findByText('{"ok":true}')).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(screen.getByRole('dialog')).toHaveAttribute('data-asset-id', 'picture');
    await waitFor(() => expect(within(screen.getByRole('dialog')).getByRole('img')).toHaveAttribute('src', 'blob:preview'));
    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(screen.getByRole('dialog')).toHaveAttribute('data-asset-id', 'clip');
    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(screen.getByRole('dialog')).toHaveAttribute('data-asset-id', 'json');
    expect(await screen.findByText('{"ok":true}')).toBeInTheDocument();
  });

  it('shows empty and failed previews without losing download access', async () => {
    preview.mockResolvedValueOnce({ text: '', truncated: false });
    preview.mockRejectedValueOnce(new Error('binary'));
    render(<MessageAttachments files={[file(), file('log')]} discussionId="disc" t={t} />);
    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentText:json.json' }));
    expect(await screen.findByText('disc.attachmentTextEmpty')).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(await screen.findByRole('alert')).toHaveTextContent('disc.attachmentTextFailed');
    expect(within(screen.getByRole('dialog')).getByRole('button', { name: 'disc.assets.downloadFor:log.json' })).toBeEnabled();
  });

  it('aborts stale requests when selecting another file or changing discussion', async () => {
    let resolveFirst!: (value: { text: string; truncated: boolean }) => void;
    preview.mockReturnValueOnce(new Promise(resolve => { resolveFirst = resolve; }));
    const { rerender } = render(<MessageAttachments files={[file(), file('next')]} discussionId="disc" t={t} />);
    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentText:json.json' }));
    expect(screen.getByRole('status')).toHaveTextContent('disc.attachmentTextLoading');
    const signal: AbortSignal = preview.mock.calls[0][2];
    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(signal.aborted).toBe(true);
    await act(async () => resolveFirst({ text: 'stale', truncated: false }));
    expect(screen.queryByText('stale')).toBeNull();
    expect(await screen.findByText('{"ok":true}')).toBeInTheDocument();
    const nextSignal: AbortSignal = preview.mock.calls[1][2];
    await act(async () => rerender(<MessageAttachments files={[file('other', { discussion_id: 'other-disc' })]} discussionId="other-disc" t={t} />));
    expect(nextSignal.aborted).toBe(true);
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});
