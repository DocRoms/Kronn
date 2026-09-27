import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { StepTokensBadge } from '../StepTokens';
import type { StepResult } from '../../../types/generated';

const t = (key: string) => key;

const result = (overrides: Partial<StepResult>): StepResult => ({
  step_name: 'orchestrate',
  status: 'Success',
  output: '',
  tokens_used: 0,
  duration_ms: 360_000,
  is_rollback: false,
  step_kind: 'Agent',
  ...overrides,
} as StepResult);

describe('StepTokensBadge', () => {
  it('shows a measured count', () => {
    render(<StepTokensBadge sr={result({ tokens_used: 48213 })} t={t} className="x" />);
    expect(screen.getByText(/48[,. \u202f]?213 wf\.stepTokensSuffix/)).toBeInTheDocument();
  });

  it('shows unknown instead of zero when the runtime reported no usage', () => {
    render(<StepTokensBadge sr={result({ tokens_used: null })} t={t} className="x" />);
    const badge = screen.getByText('wf.stepTokensUnknown');
    expect(badge).toHaveAttribute('data-unknown', 'true');
    expect(badge).toHaveAttribute('title', 'wf.stepTokensUnknownHint');
    expect(screen.queryByText(/^0/)).not.toBeInTheDocument();
  });

  it('stays clean for a measured zero and for a step still running', () => {
    const { container, rerender } = render(<StepTokensBadge sr={result({ tokens_used: 0, step_kind: 'Gate' })} t={t} className="x" />);
    expect(container).toBeEmptyDOMElement();
    rerender(<StepTokensBadge sr={result({ tokens_used: null, status: 'Running' })} t={t} className="x" />);
    expect(container).toBeEmptyDOMElement();
  });
});
