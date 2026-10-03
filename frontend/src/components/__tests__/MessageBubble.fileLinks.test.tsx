// KT-954 — a link an agent writes to a file on its machine must lead somewhere:
// the attachment it names, or plain text that says the file is not here. It
// must never become an <a> resolving against Kronn's own origin.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MarkdownContent } from '../MessageBubble';
import { MessageFileLinkContext } from '../../lib/messageFileLinkContext';
import { discussions } from '../../lib/api';
import type { ContextFile } from '../../types/generated';

const GIF = '/private/var/folders/nm/T/sax-groove-loop.gif';

function attachment(filename: string, mime_type: string): ContextFile {
  return {
    id: `file-${filename}`,
    discussion_id: 'disc-1',
    filename,
    mime_type,
    original_size: 3,
    extracted_size: 0,
    disk_path: `/data/context-files/${filename}`,
    message_id: 'msg-1',
    ai_generation: null,
    extracted_from_asset_id: null,
    created_at: '2026-10-02T00:00:00Z',
  } as unknown as ContextFile;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('MarkdownContent — links to local files', () => {
  it('keeps a web link as a link', () => {
    render(<MarkdownContent content="[docs](https://example.com/guide)" />);
    expect(screen.getByRole('link', { name: 'docs' }).getAttribute('href')).toBe(
      'https://example.com/guide',
    );
  });

  it('shows an unattached local file as plain text, never a dead link', () => {
    render(<MarkdownContent content={`[le GIF](${GIF})`} />);
    expect(screen.queryByRole('link')).toBeNull();
    const chip = screen.getByText('le GIF').closest('.disc-md-file-link');
    expect(chip?.classList.contains('disc-md-file-link--unavailable')).toBe(true);
    expect(chip?.getAttribute('data-local-path')).toBe(GIF);
    expect(chip?.getAttribute('title')).toContain('disc.localFile.notAttached');
  });

  it('opens the attachment a local link names', async () => {
    const blob = vi
      .spyOn(discussions, 'contextFileBlob')
      .mockResolvedValue(new Blob(['gif'], { type: 'image/gif' }));
    const openSpy = vi.spyOn(window, 'open').mockReturnValue(null);
    URL.createObjectURL = vi.fn(() => 'blob:sax');
    URL.revokeObjectURL = vi.fn();

    render(
      <MessageFileLinkContext.Provider value={{ attachments: [attachment('sax-groove-loop.gif', 'image/gif')] }}>
        <MarkdownContent content={`[le GIF](${GIF})`} />
      </MessageFileLinkContext.Provider>,
    );
    expect(screen.queryByRole('link')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /le GIF/ }));

    await waitFor(() => expect(blob).toHaveBeenCalledWith('disc-1', 'file-sax-groove-loop.gif'));
    await waitFor(() => expect(openSpy).toHaveBeenCalledWith('blob:sax', '_blank', 'noopener,noreferrer'));
  });

  it('shows a project path as plain text when no file is attached', () => {
    render(<MarkdownContent content="[le routeur](docs/AGENTS.md)" />);
    expect(screen.queryByRole('link')).toBeNull();
    const chip = screen.getByText('le routeur').closest('.disc-md-file-link');
    expect(chip?.getAttribute('data-local-path')).toBe('docs/AGENTS.md');
    expect(chip?.getAttribute('title')).toContain('disc.localFile.projectPath');
  });

  it('opens a project path in the project, on its line', () => {
    const onOpenProjectFile = vi.fn();
    render(
      <MessageFileLinkContext.Provider value={{ onOpenProjectFile }}>
        <MarkdownContent content="[le routeur](./docs/AGENTS.md:12)" />
      </MessageFileLinkContext.Provider>,
    );
    expect(screen.queryByRole('link')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'disc.localFile.openInProject' }));
    expect(onOpenProjectFile).toHaveBeenCalledWith('docs/AGENTS.md', 12);
  });

  it.each(['file:///tmp/out.png', 'C:/out.png'])('keeps %s through the real Markdown renderer', path => {
    render(<MarkdownContent content={`[file](${path})`} />);
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.getByText('file').closest('[data-local-path]')?.getAttribute('data-local-path')).toBe(path);
  });

  it('retains a line suffix on a root-level project filename', () => {
    const onOpenProjectFile = vi.fn();
    render(<MessageFileLinkContext.Provider value={{ onOpenProjectFile }}><MarkdownContent content="[readme](README.md:12)" /></MessageFileLinkContext.Provider>);
    fireEvent.click(screen.getByRole('button', { name: 'disc.localFile.openInProject' }));
    expect(onOpenProjectFile).toHaveBeenCalledWith('README.md', 12);
  });

  it('turns a missing project file into an unavailable chip', async () => {
    const onOpenProjectFile = vi.fn().mockResolvedValue(false);
    render(<MessageFileLinkContext.Provider value={{ onOpenProjectFile }}><MarkdownContent content="[missing](src/missing.ts)" /></MessageFileLinkContext.Provider>);
    fireEvent.click(screen.getByRole('button'));
    await waitFor(() => expect(screen.queryByRole('button')).toBeNull());
    expect(screen.getByText('missing').getAttribute('title')).toBe('disc.localFile.reason.missing');
  });

  it('renders a persisted refusal with its reason and no dead link', () => {
    render(<MarkdownContent content="[private](#kronn-local-file-unavailable-sensitive)" />);
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.getByText('private').getAttribute('title')).toBe('disc.localFile.reason.sensitive');
  });

  it('opens the exact attachment and displays an attached Markdown image inline', async () => {
    const first = { ...attachment('out.png', 'image/png'), id: 'one' };
    const second = { ...attachment('out.png', 'image/png'), id: 'two' };
    const blob = vi.spyOn(discussions, 'contextFileBlob').mockResolvedValue(new Blob(['png']));
    URL.createObjectURL = vi.fn(() => 'blob:two');
    URL.revokeObjectURL = vi.fn();
    const onOpenAttachment = vi.fn();
    const { unmount } = render(<MessageFileLinkContext.Provider value={{ attachments: [first, second], onOpenAttachment }}><MarkdownContent content="[second](/api/discussions/disc-1/context-files/two/content) ![preview](/api/discussions/disc-1/context-files/two/content)" /></MessageFileLinkContext.Provider>);
    fireEvent.click(screen.getByRole('button', { name: 'second' }));
    expect(onOpenAttachment).toHaveBeenCalledWith(second);
    const image = await screen.findByRole('img', { name: 'preview' });
    expect(image.getAttribute('src')).toBe('blob:two');
    expect(blob).toHaveBeenCalledWith('disc-1', 'two');
    unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:two');
  });
});
