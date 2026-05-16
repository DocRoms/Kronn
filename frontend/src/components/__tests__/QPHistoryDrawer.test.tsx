// 0.8.5 — QPHistoryDrawer: pure helpers + rendering smoke.
//
// Coverage:
//   - diffLines() pure helper: line-by-line classification (same /
//     changed / added / removed).
//   - Drawer hides when versionCount = 0 (legacy QP without history).
//   - Drawer renders when versionCount ≥ 1; CTA opens the panel,
//     X button + Escape close it.
//   - Version cards render with metrics chips + "current" pill on v_n.
//   - Δ% chip is hidden when launches < 3 (noise floor).
//   - Δ% chip is rendered when both versions have ≥ 3 launches.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { ReactElement } from 'react';
import type { QuickPromptVersion, QuickPromptVersionMetrics } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import QPHistoryDrawer, { diffLines } from '../QPHistoryDrawer';
import { quickPrompts as quickPromptsApi } from '../../lib/api';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const mkVersion = (n: number, body: string, when = `2026-05-1${n}T10:00:00Z`): QuickPromptVersion => ({
  id: `qpv-${n}`,
  quick_prompt_id: 'qp-X',
  version_index: n,
  name: 'Test',
  icon: '⚡',
  prompt_template: body,
  variables: [],
  agent: 'ClaudeCode',
  project_id: null,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  tier: 'default',
  description: '',
  created_at: when,
});

const mkMetrics = (
  version_index: number,
  launches: number,
  avg_tokens: number,
  avg_duration_ms: number,
): QuickPromptVersionMetrics => ({
  version_index, launches, avg_tokens, avg_duration_ms, avg_cost_usd: null,
});

beforeEach(() => { vi.clearAllMocks(); });

describe('diffLines (0.8.5)', () => {
  it('returns one row per line, kind=same when both equal', () => {
    const r = diffLines('a\nb', 'a\nb');
    expect(r).toEqual([
      { prev: 'a', next: 'a', kind: 'same' },
      { prev: 'b', next: 'b', kind: 'same' },
    ]);
  });

  it('flags changed lines', () => {
    const r = diffLines('hello world', 'hello universe');
    expect(r[0].kind).toBe('changed');
  });

  it('flags added lines (prev empty for that index)', () => {
    const r = diffLines('a', 'a\nb');
    expect(r[1].kind).toBe('added');
    expect(r[1].next).toBe('b');
    expect(r[1].prev).toBe('');
  });

  it('flags removed lines (next empty)', () => {
    const r = diffLines('a\nb', 'a');
    expect(r[1].kind).toBe('removed');
    expect(r[1].prev).toBe('b');
    expect(r[1].next).toBe('');
  });

  it('handles empty prev (entire next is added)', () => {
    const r = diffLines('', 'hello');
    expect(r).toEqual([{ prev: '', next: 'hello', kind: 'added' }]);
  });

  it('handles empty next (entire prev is removed)', () => {
    const r = diffLines('hello', '');
    expect(r).toEqual([{ prev: 'hello', next: '', kind: 'removed' }]);
  });

  it('mixed: unchanged + changed + added', () => {
    const r = diffLines('line A\nline B', 'line A\nline B-new\nline C');
    expect(r.map(x => x.kind)).toEqual(['same', 'changed', 'added']);
  });
});

describe('QPHistoryDrawer (0.8.5)', () => {
  it('renders the CTA even when versionCount = 0 (opening the drawer shows the "no versions" empty state)', async () => {
    // 0.8.5 follow-up — pre-fix the CTA was hidden when no history
    // existed, but that left users on legacy QPs with no affordance
    // at all. Post-059 backfill every QP gets v1, but during the
    // brief race window between deploy and migration-run, a transient
    // empty list shouldn't blank the button.
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    wrap(<QPHistoryDrawer qpId="qp-legacy" qpName="Legacy" />);
    await waitFor(() => {
      expect(screen.getByTestId('qp-history-toggle')).toBeInTheDocument();
    });
  });

  it('renders the CTA when versionCount >= 1', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(1, 'body v1'),
    ]);
    wrap(<QPHistoryDrawer qpId="qp-1" qpName="QP One" />);
    await waitFor(() => {
      expect(screen.getByTestId('qp-history-toggle')).toBeInTheDocument();
    });
  });

  it('CTA opens the drawer, X closes it', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(1, 'body v1'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    wrap(<QPHistoryDrawer qpId="qp-1" qpName="QP One" />);
    const cta = await screen.findByTestId('qp-history-toggle');
    fireEvent.click(cta);
    expect(await screen.findByTestId('qp-history-drawer')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('qp-history-drawer-close'));
    await waitFor(() => {
      expect(screen.queryByTestId('qp-history-drawer')).toBeNull();
    });
  });

  it('renders one card per version, newest first, "current" pill on v_n', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(3, 'v3 body'),
      mkVersion(2, 'v2 body'),
      mkVersion(1, 'v1 body'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    wrap(<QPHistoryDrawer qpId="qp-multi" qpName="Multi" />);
    const cta = await screen.findByTestId('qp-history-toggle');
    fireEvent.click(cta);
    await screen.findByTestId('qp-history-drawer');
    expect(screen.getByTestId('qp-history-version-3')).toBeInTheDocument();
    expect(screen.getByTestId('qp-history-version-2')).toBeInTheDocument();
    expect(screen.getByTestId('qp-history-version-1')).toBeInTheDocument();
    // The current pill is rendered inside the v3 card only.
    const v3Card = screen.getByTestId('qp-history-version-3');
    expect(v3Card.textContent).toMatch(/current|actuelle|actual/i);
    const v2Card = screen.getByTestId('qp-history-version-2');
    expect(v2Card.textContent).not.toMatch(/current|actuelle|actual/i);
  });

  it('Δ% chip is HIDDEN when launches < 3 (noise floor)', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(2, 'v2'),
      mkVersion(1, 'v1'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkMetrics(2, 1, 800, 3000),
      mkMetrics(1, 2, 1500, 6000),
    ]);
    wrap(<QPHistoryDrawer qpId="qp-low" qpName="Low" />);
    fireEvent.click(await screen.findByTestId('qp-history-toggle'));
    await screen.findByTestId('qp-history-drawer');
    const v2Card = screen.getByTestId('qp-history-version-2');
    // No Δ% chip with launches=1 vs launches=2 (both below the 3-launch
    // floor). Token chip should be present without a "(-N%)" suffix.
    expect(v2Card.textContent).not.toMatch(/\(-\d+%\)/);
    expect(v2Card.textContent).not.toMatch(/\(\+\d+%\)/);
  });

  it('Δ% chip is RENDERED when both versions have ≥ 3 launches', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(2, 'v2'),
      mkVersion(1, 'v1'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkMetrics(2, 5, 800, 3000),
      mkMetrics(1, 5, 1000, 4000),
    ]);
    wrap(<QPHistoryDrawer qpId="qp-ok" qpName="OK" />);
    fireEvent.click(await screen.findByTestId('qp-history-toggle'));
    await screen.findByTestId('qp-history-drawer');
    const v2Card = screen.getByTestId('qp-history-version-2');
    // (800 - 1000) / 1000 = -20%, displayed without the leading "+" sign.
    expect(v2Card.textContent).toMatch(/-20%/);
    // (3000 - 4000) / 4000 = -25%
    expect(v2Card.textContent).toMatch(/-25%/);
  });

  it('shows a trash button on archived versions, NOT on the current one', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(2, 'current body'),
      mkVersion(1, 'old body'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    wrap(<QPHistoryDrawer qpId="qp-del" qpName="Del" />);
    fireEvent.click(await screen.findByTestId('qp-history-toggle'));
    await screen.findByTestId('qp-history-drawer');
    // v1 (archived) → trash visible
    expect(screen.getByTestId('qp-history-delete-1')).toBeInTheDocument();
    // v2 (current) → trash hidden (backend would refuse + UI keeps it clean)
    expect(screen.queryByTestId('qp-history-delete-2')).toBeNull();
  });

  it('clicking the trash button calls deleteVersion + refetches history', async () => {
    // history() is called by the mount-time count pre-fetch AND by the
    // drawer-open useEffect AND by the deletion's refetch. The first two
    // see both versions; after deletion only v2 remains.
    const histMock = quickPromptsApi.history as ReturnType<typeof vi.fn>;
    histMock.mockReset();
    histMock
      .mockResolvedValueOnce([mkVersion(2, 'current'), mkVersion(1, 'old')]) // mount pre-fetch
      .mockResolvedValueOnce([mkVersion(2, 'current'), mkVersion(1, 'old')]) // drawer open
      .mockResolvedValueOnce([mkVersion(2, 'current')])                       // post-delete refetch
      .mockResolvedValue([mkVersion(2, 'current')]);                          // any later calls
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    (quickPromptsApi.deleteVersion as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    // Auto-confirm the window.confirm so the test runs synchronously.
    const origConfirm = window.confirm;
    window.confirm = vi.fn(() => true);

    wrap(<QPHistoryDrawer qpId="qp-del" qpName="Del" />);
    fireEvent.click(await screen.findByTestId('qp-history-toggle'));
    await screen.findByTestId('qp-history-drawer');
    fireEvent.click(screen.getByTestId('qp-history-delete-1'));

    await waitFor(() => {
      expect(quickPromptsApi.deleteVersion).toHaveBeenCalledWith('qp-del', 1);
    });
    await waitFor(() => {
      expect(screen.queryByTestId('qp-history-version-1')).toBeNull();
    });

    window.confirm = origConfirm;
  });

  it('declining the confirm dialog does NOT call deleteVersion', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(2, 'cur'),
      mkVersion(1, 'old'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    const origConfirm = window.confirm;
    window.confirm = vi.fn(() => false);
    wrap(<QPHistoryDrawer qpId="qp-noop" qpName="Noop" />);
    fireEvent.click(await screen.findByTestId('qp-history-toggle'));
    await screen.findByTestId('qp-history-drawer');
    fireEvent.click(screen.getByTestId('qp-history-delete-1'));
    // No deletion call should fire.
    expect(quickPromptsApi.deleteVersion).not.toHaveBeenCalled();
    window.confirm = origConfirm;
  });

  it('expanding a version reveals the diff toggle when a previous version exists', async () => {
    (quickPromptsApi.history as ReturnType<typeof vi.fn>).mockResolvedValue([
      mkVersion(2, 'after improvement'),
      mkVersion(1, 'before improvement'),
    ]);
    (quickPromptsApi.metrics as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    wrap(<QPHistoryDrawer qpId="qp-diff" qpName="Diff" />);
    fireEvent.click(await screen.findByTestId('qp-history-toggle'));
    await screen.findByTestId('qp-history-drawer');
    // Expand v2.
    const v2Header = screen.getByTestId('qp-history-version-2').querySelector('button');
    fireEvent.click(v2Header!);
    expect(screen.getByTestId('qp-history-diff-toggle-2')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('qp-history-diff-toggle-2'));
    expect(screen.getByTestId('qp-history-diff-2')).toBeInTheDocument();
  });
});
