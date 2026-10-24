// KT-1434 — SkillHistoryDrawer: the version count, the timeline, the diff of
// two neighbours, and a restore that asks first and reloads the skill.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
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

    confirm.mockReturnValueOnce(true);
    fireEvent.click(restore);
    await waitFor(() => expect(skillsApi.restoreVersion).toHaveBeenCalledWith('custom-review', 1));
    await waitFor(() => expect(onRestored).toHaveBeenCalled());
    vi.unstubAllGlobals();
  });
});
