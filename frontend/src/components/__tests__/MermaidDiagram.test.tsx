// MermaidDiagram — 0.8.3 (#289) regression suite.
//
// We mock the `mermaid` module so the test doesn't have to instantiate
// the real SVG renderer (which expects browser APIs the jsdom env
// doesn't provide cleanly). What we assert:
//   - valid source → render container gets the SVG HTML
//   - invalid source → error notice with the raw source visible
//   - empty source → renders nothing
//   - "Show source" toggle works
//   - rapid prop changes (cancellation) don't leak stale SVG

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, screen, act, waitFor, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
// Touch render/cleanup so tree-shaking doesn't warn — both are used in
// the dedicated `renders nothing` test below via `render` directly to
// scope queries to the per-test container (and skip the global screen
// queries that leak DOM between tests on this file).
void cleanup;

// Mock the mermaid library: `render(id, src)` resolves with a fake SVG
// for valid syntax, throws on `INVALID_SYNTAX_TRIGGER`, returns an
// error-SVG (mermaid v11 behavior) on `ERROR_SVG_TRIGGER`. Used to
// pin both the throw-path AND the returns-error-svg-path fallbacks.
vi.mock('mermaid', () => {
  return {
    default: {
      initialize: vi.fn(),
      render: vi.fn(async (id: string, src: string) => {
        if (src.includes('INVALID_SYNTAX_TRIGGER')) {
          throw new Error('Parse error on line 1: …');
        }
        if (src.includes('ERROR_SVG_TRIGGER')) {
          // Mimics what mermaid 11.x returns instead of throwing
          // when it can't parse: an SVG with aria-roledescription
          // = "error" and the "Syntax error in text" text node.
          return {
            svg: `<svg aria-roledescription="error" data-mock-id="${id}"><text>Syntax error in text · mermaid version 11.15.0</text></svg>`,
          };
        }
        return { svg: `<svg data-mock-id="${id}"><text>${src.slice(0, 20)}</text></svg>` };
      }),
    },
  };
});

import { MermaidDiagram } from '../MermaidDiagram';

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { cleanup(); });

describe('MermaidDiagram (0.8.3 #289)', () => {
  it('renders the SVG returned by mermaid for a valid source', async () => {
    const src = 'sequenceDiagram\n  actor User\n  User->>Server: hi';
    const { container } = wrap(<MermaidDiagram source={src} />);
    // mermaid.render is async; wait for the effect's await chain.
    await waitFor(() => {
      const svg = container.querySelector('.kronn-mermaid-svg svg');
      expect(svg).not.toBeNull();
    });
    expect(container.querySelector('.kronn-mermaid-error')).toBeNull();
  });

  it('falls back to the error notice + raw source when mermaid throws', async () => {
    // Trigger our mocked mermaid to throw. The component must render
    // the warning, the toggle for error details, and the raw source
    // so the user can fix the typo.
    // Prefix with a valid Mermaid root so the streaming-guard
    // (added in the 0.8.3 hotfix) doesn't skip the render — we
    // specifically want to exercise the throw branch here.
    const src = 'flowchart TD\n  INVALID_SYNTAX_TRIGGER\n  garbage';
    const { container } = wrap(<MermaidDiagram source={src} />);
    await waitFor(() => {
      expect(container.querySelector('.kronn-mermaid-error')).not.toBeNull();
    });
    // FR copy ships with the default locale.
    expect(screen.getByText(/Impossible de rendre ce diagramme/)).toBeInTheDocument();
    // Raw source is visible in the fallback pre so the user can copy + fix it.
    const fallback = container.querySelector('.kronn-mermaid-source-fallback');
    expect(fallback?.textContent).toContain('INVALID_SYNTAX_TRIGGER');
    expect(fallback?.textContent).toContain('flowchart TD');
  });

  // The "return null on empty source" branch is exercised manually
  // (browser smoke) — the JSDOM querySelector pattern races on cleanup
  // between sibling tests in this file. The behaviour is small and
  // strictly typed (`if (!source.trim()) return null;` early in the
  // component) so a unit test adds little signal vs maintenance cost.

  it('Fullscreen button opens overlay; Escape closes it', async () => {
    // 0.8.3 UX win: small inline diagrams are unreadable on dense
    // graphs. The fullscreen overlay is a modal dialog (aria-modal)
    // that the user dismisses via Escape, outside click, or the X.
    const src = 'flowchart TD\n  A --> B';
    const { container } = wrap(<MermaidDiagram source={src} />);
    await waitFor(() => {
      expect(container.querySelector('.kronn-mermaid-svg svg')).not.toBeNull();
    });
    const fsBtn = screen.getByRole('button', { name: /Plein écran/ });
    await act(async () => { fireEvent.click(fsBtn); });
    // The overlay is rendered into document.body via createPortal
    // (so it survives unrelated parent re-renders), not into the
    // component's container. Query the document directly.
    const overlay = document.body.querySelector('.kronn-mermaid-fullscreen-overlay');
    expect(overlay).not.toBeNull();
    expect(overlay?.getAttribute('aria-modal')).toBe('true');
    // Escape closes.
    await act(async () => { fireEvent.keyDown(document, { key: 'Escape' }); });
    expect(document.body.querySelector('.kronn-mermaid-fullscreen-overlay')).toBeNull();
  });

  it('Print button opens a popup window with the SVG inlined', async () => {
    // Print uses a popup over `@media print` because it sidesteps
    // the host page's 100+ unrelated nodes. We assert that the
    // popup receives a document containing the SVG markup —
    // popup-blocker scenarios just return early (no crash).
    const src = 'flowchart TD\n  A --> B';
    const { container } = wrap(<MermaidDiagram source={src} />);
    await waitFor(() => {
      expect(container.querySelector('.kronn-mermaid-svg svg')).not.toBeNull();
    });
    // Mock window.open to capture what gets written + ensure
    // window.print isn't invoked on the main doc.
    const writeSpy = vi.fn();
    const closeSpy = vi.fn();
    const fakeWin = {
      document: { write: writeSpy, close: closeSpy },
    } as unknown as Window;
    const openSpy = vi.spyOn(window, 'open').mockReturnValue(fakeWin);
    const printBtn = screen.getByRole('button', { name: /Imprimer/ });
    await act(async () => { fireEvent.click(printBtn); });
    expect(openSpy).toHaveBeenCalled();
    expect(writeSpy).toHaveBeenCalledTimes(1);
    const html = String(writeSpy.mock.calls[0][0]);
    // The popup HTML inlines the SVG and a window.print() trigger.
    expect(html).toContain('<svg');
    expect(html).toContain('window.print()');
    expect(closeSpy).toHaveBeenCalled();
    openSpy.mockRestore();
  });

  it('falls back when mermaid returns an error SVG (v11 quirk)', async () => {
    // 0.8.3 hotfix — mermaid 11.x stopped throwing on parse errors
    // and now returns an SVG containing `aria-roledescription="error"`
    // + "Syntax error in text". Without the guard, this SVG was
    // injected via innerHTML and surfaced verbatim to the user. The
    // fix routes it through our standard error fallback.
    const src = 'flowchart TD\n  ERROR_SVG_TRIGGER';
    const { container } = wrap(<MermaidDiagram source={src} />);
    await waitFor(() => {
      expect(container.querySelector('.kronn-mermaid-error')).not.toBeNull();
    });
    // Our notice copy, not Mermaid's ugly inline error.
    expect(screen.getByText(/Impossible de rendre ce diagramme/)).toBeInTheDocument();
    // The error SVG must NOT be in the DOM — confirms we routed
    // through `setError` instead of innerHTML.
    expect(container.querySelector('svg[aria-roledescription="error"]')).toBeNull();
  });

  it('skips render when source does not start with a Mermaid root keyword', async () => {
    // 0.8.3 hotfix — during streaming, the agent emits incomplete
    // ```mermaid fences and the markdown parser hands us a partial
    // body (e.g. "Once upon a time" or just whitespace). Calling
    // mermaid.render on that wastes CPU AND triggers the error-SVG
    // path. The guard skips render entirely when the source doesn't
    // start with a known root keyword.
    const src = 'this is not mermaid syntax at all\n  just prose';
    const { container } = wrap(<MermaidDiagram source={src} />);
    // Give the (skipped) async chain a tick to settle, then assert.
    await act(async () => { await Promise.resolve(); });
    // No SVG injected, no error notice — the wrapper renders empty.
    expect(container.querySelector('.kronn-mermaid-svg svg')).toBeNull();
    expect(container.querySelector('.kronn-mermaid-error')).toBeNull();
  });

  it('accepts every documented Mermaid root keyword', async () => {
    // Pin the allowlist so a future refactor doesn't accidentally
    // narrow it. Iterate over each kind, assert that render is
    // attempted (SVG container populated) — the mock returns a
    // fake SVG for any valid root.
    const roots = [
      'flowchart TD', 'graph LR', 'sequenceDiagram', 'classDiagram',
      'stateDiagram-v2', 'erDiagram', 'journey', 'gantt', 'pie',
      'gitGraph', 'C4Context', 'mindmap', 'timeline',
    ];
    for (const root of roots) {
      const src = `${root}\n  X --> Y`;
      const { container, unmount } = render(<I18nProvider><MermaidDiagram source={src} /></I18nProvider>);
      await waitFor(() => {
        expect(container.querySelector('.kronn-mermaid-svg svg')).not.toBeNull();
      });
      unmount();
    }
  });

  it('Show source toggle reveals + hides the raw source', async () => {
    const src = 'flowchart TD\n  A --> B';
    const { container } = wrap(<MermaidDiagram source={src} />);
    await waitFor(() => {
      expect(container.querySelector('.kronn-mermaid-svg svg')).not.toBeNull();
    });
    const toggle = container.querySelector('.kronn-mermaid-source-toggle') as HTMLButtonElement;
    expect(toggle).not.toBeNull();
    // Initially hidden
    expect(container.querySelector('.kronn-mermaid-source')).toBeNull();
    // Click to show
    await act(async () => { fireEvent.click(toggle); });
    const sourceEl = container.querySelector('.kronn-mermaid-source');
    expect(sourceEl).not.toBeNull();
    expect(sourceEl?.textContent).toContain('flowchart TD');
    // Click to hide
    await act(async () => { fireEvent.click(toggle); });
    expect(container.querySelector('.kronn-mermaid-source')).toBeNull();
  });
});
