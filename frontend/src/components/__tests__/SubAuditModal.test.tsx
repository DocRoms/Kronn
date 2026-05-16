// SubAuditModal — 0.8.4 (#287) regression suite.
//
// Validates the audit-kind picker:
//   - hidden when open=false (zero DOM footprint)
//   - exposes 1 "Full" tile + 7 targeted tiles when targetedOnly=false
//   - exposes ONLY the 7 targeted tiles when targetedOnly=true
//   - calls onPick with the kind label + then onClose on tile click
//   - Escape closes the modal
//   - clicking the backdrop closes
//   - clicking the modal body does NOT close (stopPropagation)
//   - RGAA is one of the seven targeted options + has a unique testid

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { ReactElement } from 'react';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import SubAuditModal from '../SubAuditModal';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

beforeEach(() => { vi.clearAllMocks(); });

describe('SubAuditModal (0.8.4 #287)', () => {
  it('renders nothing when open=false', () => {
    const { container } = wrap(
      <SubAuditModal open={false} onClose={() => {}} onPick={() => {}} />
    );
    expect(container.querySelector('[data-testid="sub-audit-modal"]')).toBeNull();
  });

  it('renders Full + 7 targeted kinds when targetedOnly=false', () => {
    wrap(<SubAuditModal open={true} onClose={() => {}} onPick={() => {}} />);
    expect(screen.getByTestId('sub-audit-pick-Full')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-Security')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-Docker')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-Performance')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-Accessibility')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-Rgaa')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-Database')).toBeInTheDocument();
    expect(screen.getByTestId('sub-audit-pick-ApiDesign')).toBeInTheDocument();
  });

  it('hides Full when targetedOnly=true (post-Validated entry point)', () => {
    wrap(<SubAuditModal open={true} onClose={() => {}} onPick={() => {}} targetedOnly />);
    expect(screen.queryByTestId('sub-audit-pick-Full')).toBeNull();
    // The 7 targeted kinds remain accessible.
    expect(screen.getByTestId('sub-audit-pick-Rgaa')).toBeInTheDocument();
  });

  it('calls onPick then onClose when a kind tile is clicked', () => {
    const onPick = vi.fn();
    const onClose = vi.fn();
    wrap(<SubAuditModal open={true} onClose={onClose} onPick={onPick} />);

    fireEvent.click(screen.getByTestId('sub-audit-pick-Rgaa'));
    expect(onPick).toHaveBeenCalledWith('Rgaa');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('Escape key closes the modal', () => {
    const onClose = vi.fn();
    wrap(<SubAuditModal open={true} onClose={onClose} onPick={() => {}} />);
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('backdrop click closes', () => {
    const onClose = vi.fn();
    wrap(<SubAuditModal open={true} onClose={onClose} onPick={() => {}} />);
    fireEvent.click(screen.getByTestId('sub-audit-modal'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('clicking inside the modal does NOT close (stopPropagation)', () => {
    const onClose = vi.fn();
    wrap(<SubAuditModal open={true} onClose={onClose} onPick={() => {}} />);
    fireEvent.click(screen.getByTestId('sub-audit-pick-Security').parentElement!);
    // The button itself triggers onPick → onClose, so we click the
    // surrounding grid container which has no onClick. The backdrop
    // listener should NOT fire because the stopPropagation guard runs.
    // Use the inner card div:
    const inner = screen.getByTestId('sub-audit-modal').firstElementChild as HTMLElement;
    fireEvent.click(inner);
    // The first event triggered onPick which closed. After resetting the
    // counter the inner click alone must not register a close.
    onClose.mockClear();
    fireEvent.click(inner);
    expect(onClose).not.toHaveBeenCalled();
  });
});
