import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { DocPreview } from '../DocPreview';
import { docs as docsApi } from '../../lib/api';

vi.mock('../../lib/api', () => ({
  docs: {
    generatePdf: vi.fn(),
  },
}));

beforeEach(() => {
  vi.clearAllMocks();
});

describe('DocPreview', () => {
  it('mounts a sandboxed iframe with the HTML document', () => {
    const html = '<html><body><h1>Hello</h1></body></html>';
    render(<DocPreview html={html} discussionId="disc-1" />);
    const iframe = document.querySelector('iframe.doc-preview-iframe') as HTMLIFrameElement;
    expect(iframe).not.toBeNull();
    // Sandbox attribute is empty string (no scripts, no same-origin).
    // That's the intended maximal isolation.
    expect(iframe.getAttribute('sandbox')).toBe('');
    // srcdoc is set via useEffect on mount — must carry the HTML.
    expect(iframe.srcdoc).toBe(html);
  });

  it('re-renders the iframe srcdoc when html changes', () => {
    const { rerender } = render(
      <DocPreview html="<p>v1</p>" discussionId="disc-1" />,
    );
    let iframe = document.querySelector('iframe.doc-preview-iframe') as HTMLIFrameElement;
    expect(iframe.srcdoc).toBe('<p>v1</p>');

    rerender(<DocPreview html="<p>v2</p>" discussionId="disc-1" />);
    iframe = document.querySelector('iframe.doc-preview-iframe') as HTMLIFrameElement;
    expect(iframe.srcdoc).toBe('<p>v2</p>');
  });

  it('renders the PDF button in idle state', () => {
    render(<DocPreview html="<p>x</p>" discussionId="disc-1" />);
    const btn = screen.getByRole('button', { name: /pdf/i });
    expect(btn).toBeInTheDocument();
    expect(btn).not.toBeDisabled();
  });

  it('calls docs.generatePdf with the html + discussionId on click', async () => {
    vi.mocked(docsApi.generatePdf).mockResolvedValueOnce({
      path: '/home/user/.kronn/generated/disc-1/report-abcd.pdf',
      download_url: '/api/docs/file/disc-1/report-abcd.pdf',
      size_bytes: 48000,
    });
    render(<DocPreview html="<p>hi</p>" discussionId="disc-1" />);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /pdf/i }));
    });
    expect(docsApi.generatePdf).toHaveBeenCalledWith({
      discussion_id: 'disc-1',
      html: '<p>hi</p>',
      page_size: 'A4',
    });
  });

  it('shows the download link with filename + size after successful generation', async () => {
    vi.mocked(docsApi.generatePdf).mockResolvedValueOnce({
      path: '/home/user/.kronn/generated/disc-1/jira-report-abcd.pdf',
      download_url: '/api/docs/file/disc-1/jira-report-abcd.pdf',
      size_bytes: 48213,
    });
    render(<DocPreview html="<p>x</p>" discussionId="disc-1" />);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /pdf/i }));
    });
    // Download link anchors to the API route + carries the filename
    // for the browser Save As dialog.
    const link = document.querySelector('a.doc-preview-download') as HTMLAnchorElement;
    expect(link).not.toBeNull();
    expect(link.getAttribute('href')).toBe('/api/docs/file/disc-1/jira-report-abcd.pdf');
    expect(link.getAttribute('download')).toBe('jira-report-abcd.pdf');
    expect(link.textContent).toContain('jira-report-abcd.pdf');
    expect(link.textContent).toContain('47 KB'); // 48213 / 1024 ≈ 47
  });

  it('renders an error message when the API call fails', async () => {
    vi.mocked(docsApi.generatePdf).mockRejectedValueOnce(
      new Error('Sidecar unreachable: ECONNREFUSED'),
    );
    render(<DocPreview html="<p>x</p>" discussionId="disc-1" />);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /pdf/i }));
    });
    const err = document.querySelector('.doc-preview-error');
    expect(err).not.toBeNull();
    expect(err?.textContent).toContain('Sidecar unreachable');
  });

  it('disables the button while the request is in-flight', async () => {
    // Keep the promise pending so we can catch the intermediate state.
    let resolve!: (v: { path: string; download_url: string; size_bytes: number }) => void;
    vi.mocked(docsApi.generatePdf).mockImplementationOnce(
      () => new Promise((r) => { resolve = r; }),
    );
    render(<DocPreview html="<p>x</p>" discussionId="disc-1" />);
    const btn = screen.getByRole('button', { name: /pdf/i });

    await act(async () => { fireEvent.click(btn); });
    expect(btn).toBeDisabled();

    // Complete the request → button returns to idle state.
    await act(async () => {
      resolve({
        path: '/tmp/ok.pdf',
        download_url: '/api/docs/file/disc-1/ok.pdf',
        size_bytes: 100,
      });
    });
    expect(btn).not.toBeDisabled();
  });
});
