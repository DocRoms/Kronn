/**
 * 0.8.7 — Sourcing & Anti-hallucination section unit tests.
 *
 * Pins:
 *   - mode loads from the API on mount and the dropdown reflects it (typeguard
 *     defends against a corrupted/unknown server value by falling back to "warn")
 *   - changing the dropdown POSTs and toasts on success, ROLLS BACK + error
 *     toasts on failure (no silent UI/server divergence)
 *   - picking "enforce" surfaces the preview-disclaimer toast (regression
 *     guard for the "Strict feels like a noop today" footgun)
 *   - the spec toggle fetches /api/conventions/agents-md-format-v1 once and
 *     RETRIES after a failed fetch (regression guard for "specError strands
 *     the user forever")
 *   - a11y: aria-expanded / aria-controls / aria-busy / tabIndex + the spec
 *     region uses its own label (not the trigger's, to avoid SR double-read)
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';

// vi.mock factories are hoisted above top-level `const`s, so we use
// vi.hoisted() to declare the shared mocks before the factory runs.
const { getAntiHallucinationMode, saveAntiHallucinationMode } = vi.hoisted(() => ({
  getAntiHallucinationMode: vi.fn(),
  saveAntiHallucinationMode: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  config: {
    getAntiHallucinationMode,
    saveAntiHallucinationMode,
  },
}));

// react-markdown ships ESM with optional plugins — for unit tests we stub it
// to a trivial passthrough so we only assert that the *content* reached it.
vi.mock('react-markdown', () => ({
  default: ({ children }: { children: string }) => (
    <div data-testid="rm-content">{children}</div>
  ),
}));

vi.mock('remark-gfm', () => ({ default: () => {} }));

import { AntiHallucSection } from '../settings/AntiHallucSection';

const tStub = (key: string) => key;
const toastStub = vi.fn();

function mountFetch(impl: typeof fetch) {
  globalThis.fetch = impl;
}

describe('AntiHallucSection', () => {
  beforeEach(() => {
    cleanup();
    getAntiHallucinationMode.mockClear().mockResolvedValue('warn');
    saveAntiHallucinationMode.mockClear().mockResolvedValue(undefined);
    toastStub.mockClear();
  });

  afterEach(() => {
    (globalThis as { fetch?: typeof fetch }).fetch = undefined as unknown as typeof fetch;
  });

  it('loads the current mode on mount and reflects it in the trigger', async () => {
    getAntiHallucinationMode.mockResolvedValue('enforce');
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    await waitFor(() => {
      expect(screen.getByTestId('settings-anti-hallucination-mode').textContent)
        .toContain('settings.ahModeEnforce');
    });
    expect(getAntiHallucinationMode).toHaveBeenCalledTimes(1);
  });

  it('falls back to warn if the API resolves to null/undefined', async () => {
    getAntiHallucinationMode.mockResolvedValue(undefined);
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    await waitFor(() => {
      expect(screen.getByTestId('settings-anti-hallucination-mode').textContent)
        .toContain('settings.ahModeWarn');
    });
  });

  it('typeguard: falls back to warn when the server sends an unknown string', async () => {
    getAntiHallucinationMode.mockResolvedValue('totally-not-a-mode');
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    await waitFor(() => {
      expect(screen.getByTestId('settings-anti-hallucination-mode').textContent)
        .toContain('settings.ahModeWarn');
    });
  });

  it('saves the new mode on change and toasts on success', async () => {
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    await waitFor(() => {
      expect(screen.getByTestId('settings-anti-hallucination-mode')).toBeDefined();
    });
    fireEvent.click(screen.getByTestId('settings-anti-hallucination-mode'));
    fireEvent.click(screen.getByTestId('settings-anti-hallucination-mode-option-off'));
    await waitFor(() => {
      expect(saveAntiHallucinationMode).toHaveBeenCalledWith('off');
    });
    await waitFor(() => {
      expect(toastStub).toHaveBeenCalledWith('settings.antiHallucSaved', 'success');
    });
  });

  it('rollback: on save failure restores the previous mode and surfaces an error toast', async () => {
    saveAntiHallucinationMode.mockRejectedValue(new Error('boom'));
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    await waitFor(() => {
      expect(screen.getByTestId('settings-anti-hallucination-mode').textContent)
        .toContain('settings.ahModeWarn');
    });

    fireEvent.click(screen.getByTestId('settings-anti-hallucination-mode'));
    fireEvent.click(screen.getByTestId('settings-anti-hallucination-mode-option-off'));

    await waitFor(() => {
      expect(toastStub).toHaveBeenCalledWith('settings.antiHallucSaveError', 'error');
    });
    // After rollback, the displayed mode is the original one, not the failed pick.
    expect(screen.getByTestId('settings-anti-hallucination-mode').textContent)
      .toContain('settings.ahModeWarn');
  });

  it('enforce preview footgun: selecting enforce fires the preview-disclaimer toast', async () => {
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    await waitFor(() => {
      expect(screen.getByTestId('settings-anti-hallucination-mode')).toBeDefined();
    });
    fireEvent.click(screen.getByTestId('settings-anti-hallucination-mode'));
    fireEvent.click(screen.getByTestId('settings-anti-hallucination-mode-option-enforce'));

    await waitFor(() => {
      expect(toastStub).toHaveBeenCalledWith('settings.ahEnforcePreviewToast', 'info');
    });
    expect(toastStub).toHaveBeenCalledWith('settings.antiHallucSaved', 'success');
  });

  it('renders the 3 mode explanations exactly once each', () => {
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    expect(screen.getAllByText('settings.ahExplainOff', { exact: false })).toHaveLength(1);
    expect(screen.getAllByText('settings.ahExplainWarn', { exact: false })).toHaveLength(1);
    expect(screen.getAllByText('settings.ahExplainEnforce', { exact: false })).toHaveLength(1);
  });

  it('fetches the spec once on first click and renders it via markdown', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: () => Promise.resolve('# Kronn AGENTS.md convention v1\n\nbody'),
    });
    mountFetch(fetchMock as unknown as typeof fetch);

    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    const toggle = screen.getByTestId('settings-sourcing-spec-toggle');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');

    fireEvent.click(toggle);
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });
    expect(fetchMock.mock.calls[0][0]).toBe('/api/conventions/agents-md-format-v1');

    await waitFor(() => {
      expect(screen.getByTestId('rm-content').textContent)
        .toContain('Kronn AGENTS.md convention v1');
    });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');

    // Collapsing then re-expanding must NOT refetch.
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('shows the localized error string if the spec fetch fails', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 500 });
    mountFetch(fetchMock as unknown as typeof fetch);

    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    fireEvent.click(screen.getByTestId('settings-sourcing-spec-toggle'));
    await waitFor(() => {
      expect(screen.getByTestId('settings-sourcing-spec').textContent)
        .toContain('settings.sourcingSpecError');
    });
    expect(screen.queryByTestId('rm-content')).toBeNull();
  });

  it('retries the fetch on next click after a failure (no permanent stranding)', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: false, status: 500 })
      .mockResolvedValueOnce({
        ok: true,
        text: () => Promise.resolve('# Kronn AGENTS.md convention v1\n\nrecovered'),
      });
    mountFetch(fetchMock as unknown as typeof fetch);

    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    const toggle = screen.getByTestId('settings-sourcing-spec-toggle');

    fireEvent.click(toggle);
    await waitFor(() => {
      expect(screen.getByTestId('settings-sourcing-spec').textContent)
        .toContain('settings.sourcingSpecError');
    });

    // Click again — should re-fetch (NOT just toggle the error panel).
    fireEvent.click(toggle);
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });
    await waitFor(() => {
      expect(screen.getByTestId('rm-content').textContent)
        .toContain('recovered');
    });
  });

  it('a11y: toggle is aria-controls-linked to the spec panel and announces busy state', () => {
    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    const toggle = screen.getByTestId('settings-sourcing-spec-toggle');
    expect(toggle.getAttribute('aria-controls')).toBe('settings-sourcing-spec');
    expect(toggle.getAttribute('aria-busy')).toBe('false');
  });

  it('a11y: opened spec region is keyboard-scrollable and uses its own label (no double-read with trigger)', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: () => Promise.resolve('# spec'),
    });
    mountFetch(fetchMock as unknown as typeof fetch);

    render(<AntiHallucSection toast={toastStub} t={tStub} />);
    fireEvent.click(screen.getByTestId('settings-sourcing-spec-toggle'));
    await waitFor(() => {
      const region = screen.getByTestId('settings-sourcing-spec');
      expect(region.getAttribute('role')).toBe('region');
      expect(region.getAttribute('tabindex')).toBe('0');
      // The region's label must NOT mirror the trigger's label (would cause
      // a double-announcement for SR users).
      expect(region.getAttribute('aria-label')).toBe('settings.sourcingSpecRegion');
    });
  });
});
