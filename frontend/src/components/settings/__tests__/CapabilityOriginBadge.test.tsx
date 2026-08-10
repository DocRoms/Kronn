import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { CapabilityOriginBadge, type CapabilityOrigin } from '../CapabilityOriginBadge';

const labels: Record<string, string> = {
  'config.originKronn': 'Kronn',
  'config.originPersonal': 'Personnel',
  'config.originExternal': 'Externe',
};

describe('CapabilityOriginBadge', () => {
  it.each([
    ['kronn', 'Kronn'],
    ['personal', 'Personnel'],
    ['external', 'Externe'],
  ] as const)('identifies the %s provenance', (origin, label) => {
    render(
      <CapabilityOriginBadge
        origin={origin as CapabilityOrigin}
        t={key => labels[key] ?? key}
      />,
    );

    expect(screen.getByText(label)).toHaveAttribute('data-origin', origin);
  });
});
