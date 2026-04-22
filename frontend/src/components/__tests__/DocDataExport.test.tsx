import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { DocDataExport } from '../DocDataExport';
import { docs as docsApi } from '../../lib/api';

vi.mock('../../lib/api', () => ({
  docs: {
    generateCsv: vi.fn(),
    generateXlsx: vi.fn(),
    generatePptx: vi.fn(),
  },
}));

beforeEach(() => {
  vi.clearAllMocks();
});

describe('DocDataExport', () => {
  it('summarizes a CSV payload with its row count', () => {
    render(
      <DocDataExport
        payload={{ rows: [['a', 'b'], [1, 2], [3, 4]] }}
        format="csv"
        discussionId="disc-1"
      />,
    );
    expect(screen.getByText(/3 rows/)).toBeInTheDocument();
    // The label span in the header carries the format name.
    expect(document.querySelector('.doc-data-export-label')?.textContent).toBe('CSV');
  });

  it('summarizes an XLSX payload with sheet + total row counts', () => {
    render(
      <DocDataExport
        payload={{
          sheets: [
            { name: 'Q1', rows: [['a', 'b'], [1, 2]] },
            { name: 'Q2', rows: [['c'], [3], [4]] },
          ],
        }}
        format="xlsx"
        discussionId="disc-1"
      />,
    );
    expect(screen.getByText(/2 sheets, 5 rows/)).toBeInTheDocument();
  });

  it('summarizes a PPTX payload with slide count', () => {
    render(
      <DocDataExport
        payload={{ slides: [{ title: 'A' }, { title: 'B' }] }}
        format="pptx"
        discussionId="disc-1"
      />,
    );
    expect(screen.getByText(/2 slides/)).toBeInTheDocument();
  });

  it('calls docs.generateCsv with the discussion id + payload on click', async () => {
    vi.mocked(docsApi.generateCsv).mockResolvedValueOnce({
      path: '/tmp/disc-1/data-abcd.csv',
      download_url: '/api/docs/file/disc-1/data-abcd.csv',
      size_bytes: 123,
    });
    render(
      <DocDataExport
        payload={{ rows: [['a'], [1]] }}
        format="csv"
        discussionId="disc-1"
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /export csv/i }));
    });
    expect(docsApi.generateCsv).toHaveBeenCalledWith({
      discussion_id: 'disc-1',
      rows: [['a'], [1]],
    });
  });

  it('routes to generateXlsx when format is xlsx', async () => {
    vi.mocked(docsApi.generateXlsx).mockResolvedValueOnce({
      path: '/tmp/disc-1/sheet.xlsx',
      download_url: '/api/docs/file/disc-1/sheet.xlsx',
      size_bytes: 456,
    });
    render(
      <DocDataExport
        payload={{ sheets: [{ name: 'S', rows: [['x']] }] }}
        format="xlsx"
        discussionId="disc-1"
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /export xlsx/i }));
    });
    expect(docsApi.generateXlsx).toHaveBeenCalledWith({
      discussion_id: 'disc-1',
      sheets: [{ name: 'S', rows: [['x']] }],
    });
    expect(docsApi.generateCsv).not.toHaveBeenCalled();
    expect(docsApi.generatePptx).not.toHaveBeenCalled();
  });

  it('shows the download link after a successful export', async () => {
    vi.mocked(docsApi.generatePptx).mockResolvedValueOnce({
      path: '/tmp/disc-1/deck-xyz.pptx',
      download_url: '/api/docs/file/disc-1/deck-xyz.pptx',
      size_bytes: 12000,
    });
    render(
      <DocDataExport
        payload={{ slides: [{ title: 'T' }] }}
        format="pptx"
        discussionId="disc-1"
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /export pptx/i }));
    });
    const link = document.querySelector('a.doc-preview-download') as HTMLAnchorElement;
    expect(link).not.toBeNull();
    expect(link.getAttribute('href')).toBe('/api/docs/file/disc-1/deck-xyz.pptx');
    expect(link.getAttribute('download')).toBe('deck-xyz.pptx');
  });

  it('surfaces an error inline when the API call fails', async () => {
    vi.mocked(docsApi.generateCsv).mockRejectedValueOnce(
      new Error('Sidecar unreachable'),
    );
    render(
      <DocDataExport
        payload={{ rows: [['a']] }}
        format="csv"
        discussionId="disc-1"
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /export csv/i }));
    });
    const err = document.querySelector('.doc-preview-error');
    expect(err).not.toBeNull();
    expect(err?.textContent).toContain('Sidecar unreachable');
  });
});
