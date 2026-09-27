import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DocPreviewArtifact } from '../DocPreviewArtifact';

const create = vi.hoisted(() => vi.fn());
vi.mock('../../lib/api', () => ({ pages: { create } }));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: unknown[]) => args.length ? `${key}:${args.join(',')}` : key }) }));
const html = '  <style>h1{color:red}</style>\n<h1>Équipe 🦀</h1><script>const x = "<b>";</script>\n';
const props = { html, discussionId: 'room', sourceMessageId: 'message' };
const start = () => { fireEvent.click(screen.getByRole('button', { name: 'disc.docArtifactCreate' })); fireEvent.change(screen.getByLabelText('disc.docArtifactTitle'), { target: { value: 'Mon équipe' } }); };
beforeEach(() => { create.mockReset().mockResolvedValue({ id: 'new-artifact', title: 'Mon équipe' }); });
afterEach(cleanup);

describe('DocPreviewArtifact', () => {
  it('preserves HTML and provenance, activates the library and creates once for synchronous submissions', async () => {
    const activated = vi.fn();
    window.addEventListener('kronn:pages-activated', activated);
    try {
      render(<DocPreviewArtifact {...props} />);
      start();
      expect(create).not.toHaveBeenCalled();
      const button = screen.getByRole('button', { name: 'disc.docArtifactConfirm' });
      act(() => { button.click(); button.click(); });
      expect(await screen.findByRole('link', { name: 'disc.docArtifactOpen:Mon équipe' })).toHaveAttribute('href', expect.stringContaining('#page/new-artifact'));
      expect(create).toHaveBeenCalledTimes(1);
      expect(create).toHaveBeenCalledWith({ title: 'Mon équipe', html, discussion_id: 'room', source_message_id: 'message', project_id: null, slug: null, created_by_agent: null, datasets: [] });
      expect(activated).toHaveBeenCalledOnce();
    } finally { window.removeEventListener('kronn:pages-activated', activated); }
  });

  it('shows failure, permits retry and does not claim success for another preview', async () => {
    create.mockRejectedValueOnce(new Error('origin changed'));
    const { rerender } = render(<DocPreviewArtifact {...props} />);
    start();
    fireEvent.click(screen.getByRole('button', { name: 'disc.docArtifactConfirm' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('origin changed');
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'disc.docArtifactConfirm' }));
    await screen.findByRole('link');
    rerender(<DocPreviewArtifact {...props} html="<h1>Edited HTML</h1>" />);
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'disc.docArtifactCreate' })).toBeInTheDocument();
    expect(create).toHaveBeenCalledTimes(2);
  });

  it('cancels or refuses an empty title without creating', async () => {
    render(<DocPreviewArtifact {...props} />);
    start();
    fireEvent.change(screen.getByLabelText('disc.docArtifactTitle'), { target: { value: '  ' } });
    expect(screen.getByRole('button', { name: 'disc.docArtifactConfirm' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'common.cancel' }));
    await waitFor(() => expect(screen.queryByRole('textbox')).not.toBeInTheDocument());
    expect(create).not.toHaveBeenCalled();
  });
});
