import { describe, expect, it } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import { CardHeader } from './CardHeader';
import { StatusBadge } from './StatusBadge';
import { describeHeaderContract } from '../../test/headerContract';
import { describeViolations, runAxe } from '../../test/axe';

describeHeaderContract('CardHeader', CardHeader, 'card');

describe('CardHeader', () => {
  it('names its card and carries its status badges beside the name', async () => {
    render(
      <main>
        <h1>Plugins</h1>
        <article aria-labelledby="card-github">
          <CardHeader
            titleId="card-github"
            title="GitHub"
            status={<><StatusBadge tone="success">Connected</StatusBadge><StatusBadge tone="warning">Token expires soon</StatusBadge></>}
            description="api.github.com"
          />
        </article>
      </main>,
    );
    const card = screen.getByRole('article', { name: 'GitHub' });
    const heading = screen.getByRole('heading', { level: 2, name: 'GitHub' });
    expect(card).toContainElement(heading);
    const status = card.querySelector('.ui-header-status')!;
    expect([...status.querySelectorAll('[data-tone]')].map(b => b.getAttribute('data-tone'))).toEqual(['success', 'warning']);
    // The state is written, never a colour alone.
    expect(status).toHaveTextContent('Connected');
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
  });
});
