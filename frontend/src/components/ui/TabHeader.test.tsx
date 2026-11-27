import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { TabHeader } from './TabHeader';
import { describeHeaderContract } from '../../test/headerContract';

describeHeaderContract('TabHeader', TabHeader, 'tab');

describe('TabHeader', () => {
  it('names the tab content (h3), under the h2 of its detail', () => {
    render(
      <>
        <h2>Kronn</h2>
        <TabHeader title="Skills" meta={<span>4</span>} actions={<button type="button">Add a skill</button>} />
      </>,
    );
    expect(screen.getByRole('heading', { level: 3 })).toHaveTextContent('Skills');
    expect(screen.getByRole('button', { name: 'Add a skill' })).toBeInTheDocument();
  });
});
