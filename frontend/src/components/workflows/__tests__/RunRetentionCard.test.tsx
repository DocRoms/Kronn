import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { RunRetentionCard } from '../RunRetentionCard';

const t = (key: string) => key;

afterEach(() => cleanup());

describe('RunRetentionCard', () => {
  it('starts folded on the defaults and sends each window the user sets', () => {
    const onChange = vi.fn();
    render(<RunRetentionCard value={null} onChange={onChange} t={t} />);
    expect(screen.getByText('wf.retention.summaryDefaults')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /wf\.retention\.title/ }));

    fireEvent.change(screen.getByLabelText(/wf\.retention\.noOpLabel/), { target: { value: '2' } });
    expect(onChange).toHaveBeenLastCalledWith({ no_op_hours: 2 });
    fireEvent.change(screen.getByLabelText(/wf\.retention\.failureLabel/), { target: { value: '0' } });
    expect(onChange).toHaveBeenLastCalledWith({ no_op_hours: 2, failure_days: 0 });
  });

  it('opens on an existing override and clears back to the global retention', () => {
    const onChange = vi.fn();
    render(<RunRetentionCard value={{ success_days: 7 }} onChange={onChange} t={t} />);
    const success = screen.getByLabelText(/wf\.retention\.successLabel/);
    expect(success).toHaveValue(7);
    fireEvent.change(success, { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it('never sends a negative or fractional window', () => {
    const onChange = vi.fn();
    render(<RunRetentionCard value={{ no_op_hours: 24 }} onChange={onChange} t={t} />);
    const noOp = screen.getByLabelText(/wf\.retention\.noOpLabel/);
    fireEvent.change(noOp, { target: { value: '-1' } });
    fireEvent.change(noOp, { target: { value: '1.5' } });
    expect(onChange).not.toHaveBeenCalled();
  });
});
