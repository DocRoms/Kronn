import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ListHeader } from './ListHeader';
import { describeHeaderContract } from '../../test/headerContract';

describeHeaderContract('ListHeader', ListHeader, 'list');

describe('ListHeader', () => {
  it('is the page title: the one h1, its count under it, the page actions on the right', () => {
    render(
      <ListHeader
        title="Workflows"
        description="12 workflows"
        actions={<button type="button">New workflow</button>}
      />,
    );
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Workflows');
    expect(screen.getByText('12 workflows')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'New workflow' })).toBeInTheDocument();
  });

  it('is no landmark of its own: the page keeps one banner', () => {
    const { container } = render(<ListHeader title="Workflows" />);
    expect(container.querySelector('header')).toBeNull();
    expect(screen.queryByRole('banner')).toBeNull();
  });
});
