import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SkillImportDecisions } from '../SkillImportDecisions';

const t = (key: string, ...args: (string | number)[]) => args.length > 0 ? `${key}:${args.join(',')}` : key;
const binding = { kind: 'directive' as const, source_id: 'custom-style', name: 'House style', action: 'reuse' as const, existing_id: 'custom-our-style' };

describe('SkillImportDecisions — directives and profiles (KT-1843)', () => {
  it('renders nothing when the file carries no skill, directive or profile', () => {
    const { container } = render(<SkillImportDecisions decisions={[]} choices={[]} onChoose={vi.fn()} t={t} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('lists the directives and profiles of a file that carries no skill', () => {
    render(<SkillImportDecisions decisions={[]} bindings={[binding]} choices={[]} onChoose={vi.fn()} t={t} />);
    expect(screen.getByTestId('import-bindings')).toHaveTextContent('imp.bindings.kind.directive House style imp.bindings.action.reuse');
    expect(screen.queryByText('imp.skills.title')).toBeNull();
    // Reused or created: nothing to choose.
    expect(screen.queryByRole('combobox')).toBeNull();
  });
});
