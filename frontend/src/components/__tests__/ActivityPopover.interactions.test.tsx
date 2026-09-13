import { useRef, useState } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { ActiveAuditsPopover } from '../ActiveAuditsPopover';
import { ActiveRunsPopover } from '../workflows/ActiveRunsPopover';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe.each(['audits', 'runs'] as const)('%s activity disclosure interactions', kind => {
  function Harness({ onClose = vi.fn() }: { onClose?: () => void }) {
    const [open, setOpen] = useState(false);
    const triggerRef = useRef<HTMLButtonElement>(null);
    const props = { triggerRef, onClose: () => { onClose(); setOpen(false); } };
    return <I18nProvider>
      <button ref={triggerRef} onClick={() => setOpen(value => !value)}>Activity</button>
      <button>Outside</button>
      {open && (kind === 'audits'
        ? <ActiveAuditsPopover {...props} audits={[]} projects={[]} onNavigateToProject={vi.fn()} onViewAllProjects={vi.fn()} />
        : <ActiveRunsPopover {...props} workflows={[]} onNavigateToWorkflow={vi.fn()} onViewAllWorkflows={vi.fn()} />)}
    </I18nProvider>;
  }

  it('owns Escape while focused without closing the underlying collection', async () => {
    const underlyingEscape = vi.fn();
    document.addEventListener('keydown', underlyingEscape);
    try {
      render(<Harness />);
      fireEvent.click(screen.getByRole('button', { name: 'Activity' }));
      expect(screen.getByRole('dialog')).toHaveFocus();
      fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
      expect(underlyingEscape).not.toHaveBeenCalled();
      await act(async () => { await new Promise(requestAnimationFrame); });
      expect(screen.getByRole('button', { name: 'Activity' })).toHaveFocus();
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(underlyingEscape).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener('keydown', underlyingEscape);
    }
  });

  it('dismisses on touch/pointer outside without stealing focus or retaining listeners', () => {
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    fireEvent.click(screen.getByRole('button', { name: 'Activity' }));
    const outside = screen.getByRole('button', { name: 'Outside' });
    outside.focus();
    fireEvent.pointerDown(outside, { pointerType: 'touch' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(outside).toHaveFocus();
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.pointerDown(outside);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('leaves Escape elsewhere available to the focused page', () => {
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    fireEvent.click(screen.getByRole('button', { name: 'Activity' }));
    const outside = screen.getByRole('button', { name: 'Outside' });
    outside.focus();
    fireEvent.keyDown(outside, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('bounds the overlay to the viewport and repositions after resize and scroll', () => {
    vi.stubGlobal('innerWidth', 390);
    vi.stubGlobal('innerHeight', 844);
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: 'Activity' });
    const rect = vi.spyOn(trigger, 'getBoundingClientRect').mockReturnValue(new DOMRect(300, 0, 44, 44));
    fireEvent.click(trigger);
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveStyle({ left: '22px', top: '50px', width: '360px', maxHeight: '786px' });
    vi.stubGlobal('innerWidth', 320);
    fireEvent(window, new Event('resize'));
    expect(dialog).toHaveStyle({ left: '8px', width: '304px' });
    rect.mockReturnValue(new DOMRect(20, 60, 44, 44));
    fireEvent.scroll(document);
    expect(dialog).toHaveStyle({ top: '110px', maxHeight: '726px' });
  });
});
