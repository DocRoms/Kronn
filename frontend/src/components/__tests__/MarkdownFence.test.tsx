/** Proves the `kronn-doc-preview` fence in markdown is intercepted by
 *  MarkdownContent and rendered via <DocPreview>, while every other
 *  fenced code block keeps the normal <pre><code> treatment. This is
 *  the contract between the agent (which writes the fence) and the
 *  UI (which renders the preview + export button). */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MarkdownContent } from '../MessageBubble';

// DocPreview + DocDataExport both talk to /api/docs — stub the module
// to avoid hitting the network in these pure-rendering tests.
vi.mock('../../lib/api', () => ({
  docs: {
    generatePdf: vi.fn(),
    generateDocx: vi.fn(),
    generateXlsx: vi.fn(),
    generateCsv: vi.fn(),
    generatePptx: vi.fn(),
  },
}));

describe('MarkdownContent — kronn-doc-preview fence', () => {
  it('renders DocPreview when a kronn-doc-preview fence is present', () => {
    const md = [
      'Here is the PDF preview:',
      '',
      '```kronn-doc-preview',
      '<html><body><h1>Hello</h1></body></html>',
      '```',
      '',
      'Click PDF to export.',
    ].join('\n');

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    // The DocPreview card shows a "Preview" header, a PDF button, and
    // the iframe. Any of these three is a reliable tell that we
    // successfully intercepted the fence.
    expect(screen.getByRole('button', { name: /pdf/i })).toBeInTheDocument();
    const iframe = document.querySelector('iframe.doc-preview-iframe') as HTMLIFrameElement;
    expect(iframe).not.toBeNull();
    // The iframe srcdoc carries the fence payload verbatim — this is
    // what the agent wrote and what the export button will forward to
    // /api/docs/pdf.
    expect(iframe.srcdoc).toContain('<h1>Hello</h1>');
    // The surrounding markdown still renders as normal paragraphs.
    expect(screen.getByText(/Here is the PDF preview:/)).toBeInTheDocument();
    expect(screen.getByText(/Click PDF to export/)).toBeInTheDocument();
  });

  it('does NOT render DocPreview for a normal fenced code block', () => {
    const md = [
      'Here is some code:',
      '',
      '```js',
      'console.log("hello");',
      '```',
    ].join('\n');

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    expect(screen.queryByRole('button', { name: /pdf/i })).toBeNull();
    expect(document.querySelector('iframe.doc-preview-iframe')).toBeNull();
    // The code still renders in a <pre> with its text.
    const preText = document.querySelector('pre')?.textContent ?? '';
    expect(preText).toContain('console.log');
  });

  it('falls back to a normal <pre> when discussionId is not provided', () => {
    // Without a discussion context the fence can't be exported — the
    // handler stays conservative and shows the raw HTML as code.
    const md = '```kronn-doc-preview\n<p>x</p>\n```';

    render(<MarkdownContent content={md} />);

    expect(document.querySelector('iframe.doc-preview-iframe')).toBeNull();
    // The raw HTML lands in the <pre> as code text.
    expect(document.querySelector('pre')).not.toBeNull();
  });
});

describe('MarkdownContent — kronn-doc-data fence', () => {
  it('renders DocDataExport for a well-formed CSV payload', () => {
    const md = [
      'Here is the CSV:',
      '',
      '```kronn-doc-data',
      JSON.stringify({ format: 'csv', rows: [['a', 'b'], [1, 2]] }),
      '```',
    ].join('\n');

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    expect(screen.getByRole('button', { name: /export csv/i })).toBeInTheDocument();
    // Summary reflects the 2-row payload.
    expect(screen.getByText(/2 rows/)).toBeInTheDocument();
  });

  it('renders DocDataExport for an XLSX payload with multiple sheets', () => {
    const md = [
      '```kronn-doc-data',
      JSON.stringify({
        format: 'xlsx',
        sheets: [
          { name: 'A', rows: [[1], [2]] },
          { name: 'B', rows: [[3]] },
        ],
      }),
      '```',
    ].join('\n');

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    expect(screen.getByRole('button', { name: /export xlsx/i })).toBeInTheDocument();
    expect(screen.getByText(/2 sheets, 3 rows/)).toBeInTheDocument();
  });

  it('renders DocDataExport for a PPTX payload', () => {
    const md = [
      '```kronn-doc-data',
      JSON.stringify({ format: 'pptx', slides: [{ title: 'T' }] }),
      '```',
    ].join('\n');

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    expect(screen.getByRole('button', { name: /export pptx/i })).toBeInTheDocument();
  });

  it('falls back to a normal <pre> on malformed JSON inside the fence', () => {
    const md = '```kronn-doc-data\n{ not really json\n```';

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    expect(screen.queryByRole('button', { name: /export/i })).toBeNull();
    expect(document.querySelector('pre')).not.toBeNull();
  });

  it('falls back to a normal <pre> on unknown format discriminator', () => {
    // "tsv" is not one of csv/xlsx/pptx — don't crash, just render as code.
    const md = [
      '```kronn-doc-data',
      JSON.stringify({ format: 'tsv', rows: [['a']] }),
      '```',
    ].join('\n');

    render(<MarkdownContent content={md} discussionId="disc-1" />);

    expect(screen.queryByRole('button', { name: /export/i })).toBeNull();
    expect(document.querySelector('pre')).not.toBeNull();
  });
});
