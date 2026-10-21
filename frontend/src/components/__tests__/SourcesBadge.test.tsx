import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { SourcesBadge } from '../SourcesBadge';
import type { DriftCheckResponse } from '../../types/generated';

const drift = (over: Partial<DriftCheckResponse>): DriftCheckResponse => ({
  audit_date: null,
  stale_sections: [],
  fresh_sections: [],
  total_sections: 0,
  sources_state: 'unknown',
  broken_citations: 0,
  placeholder_docs: [],
  ...over,
});

const wrap = (value: DriftCheckResponse | undefined) =>
  render(<I18nProvider><SourcesBadge drift={value} /></I18nProvider>);

describe('SourcesBadge', () => {
  it('stays hidden while no fingerprint was recorded', () => {
    wrap(undefined);
    expect(screen.queryByTestId('project-sources-state')).toBeNull();
    wrap(drift({}));
    expect(screen.queryByTestId('project-sources-state')).toBeNull();
  });

  it('shows clean sources', () => {
    wrap(drift({ sources_state: 'clean' }));
    expect(screen.getByTestId('project-sources-state')).toHaveAttribute('data-state', 'clean');
  });

  it('turns to re-verify and counts the broken citations', () => {
    wrap(drift({ sources_state: 'needs_reverification', broken_citations: 3 }));
    const badge = screen.getByTestId('project-sources-state');
    expect(badge).toHaveAttribute('data-state', 'needs_reverification');
    expect(badge).toHaveTextContent('3');
  });
});
