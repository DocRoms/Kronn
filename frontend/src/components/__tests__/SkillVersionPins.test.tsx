// KT-1447 — a custom skill can be pinned to a version and set back to the
// latest; built-in skills get no selector.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { Skill, SkillVersion } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import SkillVersionPins from '../SkillVersionPins';
import { skills as skillsApi } from '../../lib/api';
import { skillBaseId, skillPinnedVersion, pickedButNotOffered } from '../../lib/automationSkills';

const skill = (id: string, name: string): Skill => ({
  id, name, description: '', icon: '🔎', category: 'Domain', content: '', is_builtin: !id.startsWith('custom-'), token_estimate: 0,
});
const version = (n: number, current = false): SkillVersion => ({
  skill_id: 'custom-review', version_index: n, name: 'Review', description: '', icon: '🔎',
  category: 'Domain', content: `v${n}`, created_at: '2026-10-23T10:00:00Z', current,
});

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(skillsApi.history).mockResolvedValue([version(2, true), version(1)]);
});

describe('skill pins (KT-1447)', () => {
  it('reads the base id and the pinned version', () => {
    expect(skillBaseId('custom-review@2')).toBe('custom-review');
    expect(skillPinnedVersion('custom-review@2')).toBe(2);
    expect(skillPinnedVersion('custom-review')).toBeNull();
    expect(skillBaseId('rust')).toBe('rust');
    const catalog = [skill('custom-review', 'Review')];
    expect(pickedButNotOffered(['custom-review@1'], catalog, catalog, [])).toEqual([]);
  });

  it('pins a version, then goes back to the latest', async () => {
    const onChange = vi.fn();
    const catalog = [skill('custom-review', 'Review'), skill('rust', 'Rust')];
    const { rerender } = render(
      <I18nProvider><SkillVersionPins ids={['rust', 'custom-review']} catalog={catalog} onChange={onChange} /></I18nProvider>,
    );
    const select = within(await screen.findByTestId('skill-version-pin-custom-review'));
    await waitFor(() => expect(select.getAllByRole('option')).toHaveLength(3));
    fireEvent.change(select.getByRole('combobox'), { target: { value: '1' } });
    expect(onChange).toHaveBeenLastCalledWith(['rust', 'custom-review@1']);
    expect(screen.queryByTestId('skill-version-pin-rust')).toBeNull();

    rerender(
      <I18nProvider><SkillVersionPins ids={['rust', 'custom-review@1']} catalog={catalog} onChange={onChange} /></I18nProvider>,
    );
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith(['rust', 'custom-review']);
  });
});

