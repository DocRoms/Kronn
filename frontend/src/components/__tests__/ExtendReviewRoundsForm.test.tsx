import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { ExtendReviewRoundsForm } from '../ExtendReviewRoundsForm';
import { UNLIMITED_REVIEW_ROUNDS } from '../../lib/reviewRounds';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join(',')})` : key;

afterEach(cleanup);

const renderForm = (over: Partial<React.ComponentProps<typeof ExtendReviewRoundsForm>> = {}) => {
  const props = {
    t,
    reviewRounds: 3,
    maxReviewRounds: 3,
    onConfirm: vi.fn(),
    onCancel: vi.fn(),
    ...over,
  };
  render(<ExtendReviewRoundsForm {...props} />);
  return props;
};

describe('ExtendReviewRoundsForm (KT-1345)', () => {
  it('shows rounds used / limit and extends by the chosen number', () => {
    const props = renderForm();
    expect(screen.getByTestId('orch-extend-current')).toHaveTextContent('orch.extend.current(3,3)');
    fireEvent.change(screen.getByTestId('orch-extend-by'), { target: { value: '4' } });
    fireEvent.click(screen.getByTestId('orch-extend-confirm'));
    expect(props.onConfirm).toHaveBeenCalledWith(4);
  });

  it('shows the token-burn warning for unlimited and sends "unlimited"', () => {
    const props = renderForm();
    expect(screen.queryByTestId('orch-extend-warning')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('orch-extend-unlimited'));
    expect(screen.getByTestId('orch-extend-warning')).toHaveTextContent('config.workers.unlimitedWarning');
    fireEvent.click(screen.getByTestId('orch-extend-confirm'));
    expect(props.onConfirm).toHaveBeenCalledWith('unlimited');
  });

  it('refuses 0 and values above the cap', () => {
    const props = renderForm();
    for (const bad of ['0', '', '1001']) {
      fireEvent.change(screen.getByTestId('orch-extend-by'), { target: { value: bad } });
      expect(screen.getByTestId('orch-extend-confirm')).toBeDisabled();
    }
    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it('renders the unlimited sentinel as a symbol, not as a number', () => {
    renderForm({ maxReviewRounds: UNLIMITED_REVIEW_ROUNDS });
    expect(screen.getByTestId('orch-extend-current')).toHaveTextContent('orch.extend.current(3,∞)');
  });
});
