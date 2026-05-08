// Auto-update banner regression tests. Pin the dismiss + version-gate
// behaviour so a refactor doesn't either (a) nag users on every load
// after they dismiss, or (b) keep the banner suppressed forever even
// after a new release ships.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { UpdateBanner } from '../UpdateBanner';
import { I18nProvider } from '../../lib/I18nContext';

// vi.mock() is hoisted to the top of the file *before* any const
// declarations. Use vi.hoisted() so the spy lives in the hoisted
// scope and is reachable from both the factory and the tests below.
const mocks = vi.hoisted(() => ({
  versionCheck: vi.fn(),
  getUiLanguage: vi.fn().mockResolvedValue('fr'),
}));
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    version: { check: mocks.versionCheck },
    config: { getUiLanguage: mocks.getUiLanguage },
  };
});
const versionCheck = mocks.versionCheck;

describe('UpdateBanner', () => {
  beforeEach(() => {
    versionCheck.mockReset();
    try { localStorage.clear(); } catch { /* incognito */ }
  });

  afterEach(() => {
    try { localStorage.clear(); } catch { /* incognito */ }
  });

  it('renders nothing while the version check is in flight', () => {
    versionCheck.mockReturnValue(new Promise(() => { /* never resolves */ }));
    const { container } = render(
      <I18nProvider><UpdateBanner /></I18nProvider>
    );
    // The banner is the only child; an in-flight check = nothing rendered.
    expect(container.querySelector('.kronn-update-banner')).toBeNull();
  });

  it('renders nothing when the backend reports up_to_date', async () => {
    versionCheck.mockResolvedValue({
      current: '0.7.1', latest: '0.7.1', release_url: 'https://x', up_to_date: true,
    });
    const { container } = render(
      <I18nProvider><UpdateBanner /></I18nProvider>
    );
    // Wait for the promise microtask + state update.
    await waitFor(() => expect(versionCheck).toHaveBeenCalled());
    expect(container.querySelector('.kronn-update-banner')).toBeNull();
  });

  it('renders nothing when the backend reports latest=null (offline)', async () => {
    versionCheck.mockResolvedValue({
      current: '0.7.1', latest: null, release_url: null, up_to_date: true,
    });
    const { container } = render(
      <I18nProvider><UpdateBanner /></I18nProvider>
    );
    await waitFor(() => expect(versionCheck).toHaveBeenCalled());
    expect(container.querySelector('.kronn-update-banner')).toBeNull();
  });

  it('renders the banner with current+latest when an update is available', async () => {
    versionCheck.mockResolvedValue({
      current: '0.7.1', latest: '0.7.2', release_url: 'https://github.com/x/y/releases/v0.7.2', up_to_date: false,
    });
    render(<I18nProvider><UpdateBanner /></I18nProvider>);
    // The banner content includes both versions. We assert on the
    // pill node + text containing both numbers.
    const banner = await screen.findByRole('status');
    expect(banner).toHaveTextContent('0.7.1');
    expect(banner).toHaveTextContent('0.7.2');
    // External link to the release page.
    const link = banner.querySelector('a');
    expect(link).not.toBeNull();
    expect(link?.getAttribute('href')).toBe('https://github.com/x/y/releases/v0.7.2');
    expect(link?.getAttribute('target')).toBe('_blank');
  });

  it('hides the banner after the dismiss button is clicked', async () => {
    versionCheck.mockResolvedValue({
      current: '0.7.1', latest: '0.7.2', release_url: 'https://x', up_to_date: false,
    });
    const { container } = render(
      <I18nProvider><UpdateBanner /></I18nProvider>
    );
    await screen.findByRole('status');

    fireEvent.click(screen.getByLabelText(/Masquer|Dismiss|Ocultar/));
    // After dismiss, the banner should be gone.
    expect(container.querySelector('.kronn-update-banner')).toBeNull();
    // …and the dismissed-version sticks in localStorage.
    expect(localStorage.getItem('kronn:update-dismissed-version')).toBe('0.7.2');
  });

  it('stays hidden on remount if the SAME version was dismissed', async () => {
    localStorage.setItem('kronn:update-dismissed-version', '0.7.2');
    versionCheck.mockResolvedValue({
      current: '0.7.1', latest: '0.7.2', release_url: 'https://x', up_to_date: false,
    });
    const { container } = render(
      <I18nProvider><UpdateBanner /></I18nProvider>
    );
    await waitFor(() => expect(versionCheck).toHaveBeenCalled());
    expect(container.querySelector('.kronn-update-banner')).toBeNull();
  });

  it('shows again when a NEWER version ships, even if a previous one was dismissed', async () => {
    // Regression: dismissing 0.7.2 must not suppress the banner for
    // 0.7.3 — otherwise users miss every release after the first one
    // they ignored.
    localStorage.setItem('kronn:update-dismissed-version', '0.7.2');
    versionCheck.mockResolvedValue({
      current: '0.7.1', latest: '0.7.3', release_url: 'https://x', up_to_date: false,
    });
    render(<I18nProvider><UpdateBanner /></I18nProvider>);
    const banner = await screen.findByRole('status');
    expect(banner).toHaveTextContent('0.7.3');
  });

  it('renders nothing when the version check throws (network down)', async () => {
    versionCheck.mockRejectedValue(new Error('ECONNREFUSED'));
    // Suppress the expected console.warn so the test output stays clean.
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { container } = render(
      <I18nProvider><UpdateBanner /></I18nProvider>
    );
    await waitFor(() => expect(versionCheck).toHaveBeenCalled());
    expect(container.querySelector('.kronn-update-banner')).toBeNull();
    expect(warnSpy).toHaveBeenCalled();
    warnSpy.mockRestore();
  });
});
