import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SectionHeader } from './SectionHeader';
import { describeHeaderContract } from '../../test/headerContract';

describeHeaderContract('SectionHeader', SectionHeader, 'section');

describe('SectionHeader', () => {
  it('names a group of content (h3) and can name its region', () => {
    render(
      <section aria-labelledby="mcp-title">
        <SectionHeader titleId="mcp-title" title="MCP servers" description="Tools the agent may call" />
        <p>…</p>
      </section>,
    );
    expect(screen.getByRole('region', { name: 'MCP servers' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 3 })).toHaveTextContent('MCP servers');
  });
});
