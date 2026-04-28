// 0.7.0 Phase 1 — execution limits (guards) card UI test.
// Validates: defaults shown as placeholders, expansion triggered on
// existing overrides, onChange emits null when all fields blank, save-time
// values are converted (minutes → seconds for timeout).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ExecutionLimitsCard } from '../ExecutionLimitsCard';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

describe('ExecutionLimitsCard', () => {
  it('starts collapsed when no overrides are set', () => {
    render(<ExecutionLimitsCard value={null} onChange={() => {}} t={t} />);
    // Title visible, but inputs hidden until expanded.
    expect(screen.getByText('wf.guards.title')).toBeInTheDocument();
    expect(screen.queryByText('wf.guards.timeoutLabel')).not.toBeInTheDocument();
  });

  it('starts expanded when overrides exist', () => {
    render(
      <ExecutionLimitsCard
        value={{ timeout_seconds: 1800, max_llm_calls: null, loop_detection_max_revisits: null }}
        onChange={() => {}}
        t={t}
      />
    );
    expect(screen.getByText('wf.guards.timeoutLabel')).toBeInTheDocument();
  });

  it('shows the default summary when collapsed and no overrides', () => {
    render(<ExecutionLimitsCard value={null} onChange={() => {}} t={t} />);
    // wf.guards.summaryDefaults:120,100,10 — interpolated via test t()
    expect(screen.getByText(/wf\.guards\.summaryDefaults:120,100,10/)).toBeInTheDocument();
  });

  it('shows "custom limits" summary when at least one field is set', () => {
    render(
      <ExecutionLimitsCard
        value={{ timeout_seconds: 600, max_llm_calls: null, loop_detection_max_revisits: null }}
        onChange={() => {}}
        t={t}
      />
    );
    expect(screen.getByText(/wf\.guards\.summaryActive/)).toBeInTheDocument();
  });

  it('renders timeout in minutes (seconds → minutes conversion)', () => {
    render(
      <ExecutionLimitsCard
        value={{ timeout_seconds: 1800, max_llm_calls: null, loop_detection_max_revisits: null }}
        onChange={() => {}}
        t={t}
      />
    );
    const input = screen.getByPlaceholderText('120') as HTMLInputElement;
    expect(input.value).toBe('30'); // 1800s = 30min
  });

  it('emits onChange with seconds when user types minutes', () => {
    const onChange = vi.fn();
    render(<ExecutionLimitsCard value={null} onChange={onChange} t={t} />);
    // Expand by clicking the toggle
    fireEvent.click(screen.getByText('wf.guards.title'));
    const timeoutInput = screen.getByPlaceholderText('120') as HTMLInputElement;
    fireEvent.change(timeoutInput, { target: { value: '5' } });
    expect(onChange).toHaveBeenCalled();
    const last = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(last.timeout_seconds).toBe(300); // 5 minutes = 300 seconds
  });

  it('emits null (no overrides) when all fields are cleared', () => {
    const onChange = vi.fn();
    render(
      <ExecutionLimitsCard
        value={{ timeout_seconds: 600, max_llm_calls: null, loop_detection_max_revisits: null }}
        onChange={onChange}
        t={t}
      />
    );
    const timeoutInput = screen.getByPlaceholderText('120') as HTMLInputElement;
    fireEvent.change(timeoutInput, { target: { value: '' } });
    const last = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(last).toBeNull();
  });

  it('rejects 0 / negative values (kept blank, treated as no override)', () => {
    const onChange = vi.fn();
    render(<ExecutionLimitsCard value={null} onChange={onChange} t={t} />);
    fireEvent.click(screen.getByText('wf.guards.title'));
    const maxCallsInput = screen.getByPlaceholderText('100') as HTMLInputElement;
    fireEvent.change(maxCallsInput, { target: { value: '0' } });
    const last = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    // 0 should not be persisted — treated as "no override"
    expect(last?.max_llm_calls ?? null).toBeNull();
  });
});
