// KT-1434 — SkillHistoryDrawer: the version count, the timeline, the diff of
// two neighbours, and a restore that asks first and reloads the skill.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { ReactElement } from 'react';
import type { SkillVersion } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import SkillHistoryDrawer from '../SkillHistoryDrawer';
import { skills as skillsApi } from '../../lib/api';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const version = (n: number, content: string, current = false): SkillVersion => ({
  skill_id: 'custom-review',
  version_index: n,
  name: 'Review',
  description: '',
  icon: '🔎',
  category: 'Domain',
  content,
  created_at: `2026-10-2${n}T10:00:00Z`,
  current,
});

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(skillsApi.history).mockResolvedValue([
    version(2, 'rule one\nrule two', true),
    version(1, 'rule one'),
  ]);
});

describe('SkillHistoryDrawer (KT-1434)', () => {
  it('counts the versions and shows the diff of a version with the previous one', async () => {
    wrap(<SkillHistoryDrawer skillId="custom-review" skillName="Review" refreshKey="a" />);
    await waitFor(() => expect(screen.getByTestId('skill-history-toggle').textContent).toContain('2'));
    fireEvent.click(screen.getByTestId('skill-history-toggle'));
    const current = await screen.findByTestId('skill-history-version-2');
    expect(screen.queryByTestId('skill-history-restore-2')).toBeNull();
    fireEvent.click(current.querySelector('.qph-version-card-header') as Element);
    fireEvent.click(screen.getByTestId('skill-history-diff-toggle-2'));
    expect(screen.getByTestId('skill-history-diff-2').textContent).toContain('rule two');
  });

  it('restores an old version only after confirmation, then reloads the skill', async () => {
    const onRestored = vi.fn();
    const confirm = vi.fn();
    vi.stubGlobal('confirm', confirm);
    wrap(<SkillHistoryDrawer skillId="custom-review" skillName="Review" refreshKey="a" onRestored={onRestored} />);
    fireEvent.click(screen.getByTestId('skill-history-toggle'));
    const restore = await screen.findByTestId('skill-history-restore-1');

    confirm.mockReturnValueOnce(false);
    fireEvent.click(restore);
    expect(skillsApi.restoreVersion).not.toHaveBeenCalled();

    await act(async () => {});
    confirm.mockReturnValue(true);
    vi.mocked(skillsApi.restoreVersion).mockResolvedValue(version(3, 'rule one', true));
    const before = vi.mocked(skillsApi.history).mock.calls.length;
    fireEvent.click(restore);
    fireEvent.click(restore);
    await waitFor(() => expect(skillsApi.restoreVersion).toHaveBeenCalledWith('custom-review', 1));
    expect(skillsApi.restoreVersion).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(onRestored).toHaveBeenCalled());
    // It says what happened, and reads its own history again.
    expect(await screen.findByTestId('skill-history-restored')).toHaveTextContent('v1 restaurée');
    expect(screen.getByTestId('skill-history-restored')).toHaveTextContent('v3');
    await waitFor(() => expect(vi.mocked(skillsApi.history).mock.calls.length).toBeGreaterThan(before));
    vi.unstubAllGlobals();
  });
});

describe('SkillHistoryDrawer dialog (KT-1434)', () => {
  it('renders in the document body as a modal dialog, closed by Escape', async () => {
    const { container } = wrap(<div style={{ transform: 'scale(1)' }}><SkillHistoryDrawer skillId="custom-review" skillName="Review" refreshKey="a" /></div>);
    fireEvent.click(screen.getByTestId('skill-history-toggle'));
    const drawer = await screen.findByTestId('skill-history-drawer');
    expect(container.contains(drawer)).toBe(false);
    expect(drawer).toHaveAttribute('aria-modal', 'true');
    fireEvent.keyDown(drawer, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByTestId('skill-history-drawer')).toBeNull());
  });
});
