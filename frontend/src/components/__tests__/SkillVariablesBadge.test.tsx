// The "Variabilisé" badge (KT-906): shown for a skill declaring Claude Code
// `arguments`, with the same label and hover in every skill list.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import type { Skill } from '../../types/generated';
import { dictionaries } from '../../lib/i18n/testing';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) => (args.length > 0 ? `${key}:${args.join('|')}` : key),
  }),
}));

import { SkillVariablesBadge } from '../SkillVariablesBadge';
import { isVariabilizedSkill } from '../../lib/automationSkills';
import { SkillCard } from '../SkillSheet';
import { ProjectSkills } from '../ProjectSkills';

vi.mock('../../lib/api', () => ({ projects: { setDefaultSkills: vi.fn() } }));

const skill = (over: Partial<Skill> = {}): Skill => ({
  id: 'review', name: 'Review', description: 'Checks pull requests', icon: '🔍', category: 'Domain',
  content: 'Review $ticket.', is_builtin: false, token_estimate: 10, ...over,
});

afterEach(cleanup);

describe('SkillVariablesBadge', () => {
  it('stays hidden for a skill without arguments', () => {
    expect(isVariabilizedSkill(skill())).toBe(false);
    expect(isVariabilizedSkill(skill({ arguments: [] }))).toBe(false);
    const { container } = render(<SkillVariablesBadge skill={skill()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('names the placeholders in its hover', () => {
    render(<SkillVariablesBadge skill={skill({ arguments: ['ticket', 'from-lang'] })} />);
    const badge = screen.getByTestId('skill-variables-badge');
    expect(badge).toHaveTextContent('skills.variabilized');
    expect(badge).toHaveAttribute('title', 'skills.variabilizedHint:$ticket $from-lang');
  });

  it('is rendered in the skill lists', () => {
    const variabilized = skill({ arguments: ['ticket'] });
    render(<SkillCard skill={variabilized} pinned={false} onTogglePinned={() => {}} projectCount={0} onOpen={() => {}} />);
    expect(screen.getByTestId('skill-variables-badge')).toBeInTheDocument();
    cleanup();
    render(
      <ProjectSkills
        projectId="p"
        currentSkillIds={[]}
        allSkills={[variabilized, skill({ id: 'plain', name: 'Plain' })]}
        onUpdate={() => {}}
      />,
    );
    expect(screen.getAllByTestId('skill-variables-badge')).toHaveLength(1);
  });

  it('has its label and hover in every locale', () => {
    for (const [locale, dict] of Object.entries(dictionaries)) {
      const entries = dict as Record<string, string>;
      expect(entries['skills.variabilized'], locale).toBeTruthy();
      expect(entries['skills.variabilizedHint'], locale).toContain('{0}');
    }
  });
});
