import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { RunStatusCard, workflowRunStatusCardModel } from '../RunStatusCard';

describe('RunStatusCard', () => {
  it('renders only measured workflow progress and duration', () => {
    const model = workflowRunStatusCardModel({
      id: 'run-1',
      status: 'Running',
      started_at: '2026-08-31T10:00:00Z',
      finished_at: null,
      step_results: [
        { step_name: 'collect', status: 'Success' },
        { step_name: 'publish', status: 'Running' },
      ],
    });

    render(<RunStatusCard model={model} />);

    expect(screen.getByTestId('run-status-card')).toHaveAttribute('data-kind', 'workflow');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '1');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuemax', '2');
    expect(screen.getByText('publish')).toBeInTheDocument();
  });

  it('makes unavailable duration and diagnostics explicit for a failed direct run', () => {
    render(
      <RunStatusCard
        model={{
          id: 'qa-1',
          kind: 'quick_api',
          status: 'preflight_failed',
          diagnostic: 'The configured API is unavailable.',
          freshness: 'unavailable',
        }}
      />,
    );

    expect(screen.getByText('run.durationUnavailable')).toBeInTheDocument();
    expect(screen.getByText('The configured API is unavailable.')).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  });
});
