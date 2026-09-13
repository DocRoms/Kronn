import { useRef, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import type { WorkflowSummary } from '../../../types/generated';
import { ActiveRunsPopover } from '../ActiveRunsPopover';

const { cancelRunMock } = vi.hoisted(() => ({ cancelRunMock: vi.fn() }));
vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock({ workflows: { cancelRun: cancelRunMock as never } });
});

const workflow = {
  id: 'workflow-1', name: 'Release', project_id: null, project_name: null,
  trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0, enabled: true, pinned: false,
  last_run: { id: 'run-1', status: 'Running', started_at: '2026-01-01T00:00:00Z', finished_at: null, tokens_used: 0 },
  created_at: '2026-01-01T00:00:00Z',
} as const satisfies WorkflowSummary;

function renderPopover(onClose = vi.fn(), onAfterCancel = vi.fn()) {
  function Harness() {
    const triggerRef = useRef<HTMLButtonElement>(null);
    return <I18nProvider>
      <button ref={triggerRef}>activity</button>
      <button>Outside</button>
      <ActiveRunsPopover workflows={[workflow]} triggerRef={triggerRef} onClose={onClose}
        onNavigateToWorkflow={vi.fn()} onViewAllWorkflows={vi.fn()} onAfterCancel={onAfterCancel} />
    </I18nProvider>;
  }
  render(<Harness />);
  return { onClose, trigger: screen.getByRole('button', { name: 'activity' }) };
}

beforeEach(() => {
  cancelRunMock.mockReset();
  cancelRunMock.mockResolvedValue({ run_cancelled: true, child_discs_cancelled: 0 });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ActiveRunsPopover', () => {
  it('focuses the dialog, ignores its trigger as an outside click, and restores focus on Escape', async () => {
    const { onClose, trigger } = renderPopover();
    const dialog = screen.getByRole('dialog', { name: 'Runs en cours' });
    expect(dialog).toHaveFocus();

    await act(async () => { fireEvent.pointerDown(trigger); });
    expect(onClose).not.toHaveBeenCalled();

    await act(async () => { fireEvent.keyDown(document, { key: 'Escape' }); });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(trigger).toHaveFocus();
  });

  it('closes on an outside click without taking focus and synchronously guards duplicate Stop clicks', async () => {
    let rejectCancel!: (error: Error) => void;
    cancelRunMock.mockImplementation(() => new Promise((_, reject) => { rejectCancel = reject; }));
    const onAfterCancel = vi.fn();
    const { onClose } = renderPopover(vi.fn(), onAfterCancel);
    const stop = screen.getByRole('button', { name: 'Arrêter' });

    await act(async () => { fireEvent.click(stop); fireEvent.click(stop); });
    expect(cancelRunMock).toHaveBeenCalledTimes(1);
    await act(async () => { rejectCancel(new Error('offline')); });
    expect(await screen.findByRole('alert')).toHaveTextContent("Échec de l'arrêt");
    expect(onAfterCancel).toHaveBeenCalledTimes(1);
    cancelRunMock.mockResolvedValue({ run_cancelled: true, child_discs_cancelled: 0 });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Arrêter' })); });
    expect(cancelRunMock).toHaveBeenCalledTimes(2);
    expect(onAfterCancel).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    const outside = screen.getByRole('button', { name: 'Outside' });
    outside.focus();
    await act(async () => { fireEvent.pointerDown(outside); });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(outside).toHaveFocus();
  });

  it('lists only Running and Pending runs and otherwise shows the empty state', () => {
    const idle: WorkflowSummary = { ...workflow, id: 'idle', name: 'Idle', last_run: { ...workflow.last_run, status: 'Success' } };
    const pending: WorkflowSummary = { ...workflow, id: 'pending', name: 'Pending', last_run: { ...workflow.last_run, id: 'run-pending', status: 'Pending' } };
    render(<I18nProvider><ActiveRunsPopover workflows={[idle, pending]} onClose={vi.fn()} onNavigateToWorkflow={vi.fn()} onViewAllWorkflows={vi.fn()} /></I18nProvider>);
    expect(screen.getByText('Pending')).toBeInTheDocument();
    expect(screen.queryByText('Idle')).not.toBeInTheDocument();
    cleanup();
    render(<I18nProvider><ActiveRunsPopover workflows={[idle]} onClose={vi.fn()} onNavigateToWorkflow={vi.fn()} onViewAllWorkflows={vi.fn()} /></I18nProvider>);
    expect(screen.getByText('Aucun run en cours')).toBeInTheDocument();
  });

  it('cancels the exact workflow/run pair and disables the Stop button while pending', async () => {
    cancelRunMock.mockImplementation(() => new Promise(() => {}));
    renderPopover();
    const stop = screen.getByRole('button', { name: 'Arrêter' });
    fireEvent.click(stop);
    await waitFor(() => expect(cancelRunMock).toHaveBeenCalledWith('workflow-1', 'run-1'));
    expect(screen.getByRole('button', { name: 'Arrêt…' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Arrêt…' }));
    expect(cancelRunMock).toHaveBeenCalledTimes(1);
  });

  it('does not navigate when Stop is clicked, but navigates from the row', async () => {
    const onNavigate = vi.fn();
    render(<I18nProvider><ActiveRunsPopover workflows={[workflow]} onClose={vi.fn()} onNavigateToWorkflow={onNavigate} onViewAllWorkflows={vi.fn()} /></I18nProvider>);
    fireEvent.click(screen.getByRole('button', { name: 'Arrêter' }));
    await waitFor(() => expect(cancelRunMock).toHaveBeenCalled());
    expect(onNavigate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /Release/ }));
    expect(onNavigate).toHaveBeenCalledWith('workflow-1');
  });

  it('navigates from the footer', () => {
    const onViewAll = vi.fn();
    render(<I18nProvider><ActiveRunsPopover workflows={[]} onClose={vi.fn()} onNavigateToWorkflow={vi.fn()} onViewAllWorkflows={onViewAll} /></I18nProvider>);
    fireEvent.click(screen.getByRole('button', { name: 'Voir tous les workflows' }));
    expect(onViewAll).toHaveBeenCalledTimes(1);
  });

  it('closes from the close button and restores focus to its trigger', async () => {
    const { trigger } = renderPopover();
    fireEvent.click(screen.getByRole('button', { name: 'Fermer' }));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(trigger).toHaveFocus();
  });

  it('does not close when the pointer starts within the dialog', async () => {
    const { onClose } = renderPopover();
    await act(async () => { fireEvent.pointerDown(screen.getByRole('dialog')); });
    expect(onClose).not.toHaveBeenCalled();
  });

  it('falls back to the surviving workflow tab when a zero-count close unmounts its trigger', async () => {
    function Harness() {
      const [open, setOpen] = useState(true);
      const triggerRef = useRef<HTMLButtonElement>(null);
      const tabRef = useRef<HTMLButtonElement>(null);
      return <>
        <button ref={tabRef} type="button">Automation</button>
        {open && <button ref={triggerRef} type="button">activity</button>}
        {open && <ActiveRunsPopover workflows={[]} triggerRef={triggerRef} focusFallbackRef={tabRef} onClose={() => setOpen(false)} onNavigateToWorkflow={vi.fn()} onViewAllWorkflows={vi.fn()} />}
      </>;
    }
    render(<I18nProvider><Harness /></I18nProvider>);
    await act(async () => { fireEvent.keyDown(document, { key: 'Escape' }); });
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(screen.getByRole('button', { name: 'Automation' })).toHaveFocus();
  });

  it('keeps keyboard focus in the dialog when the last focused run row disappears', async () => {
    const props = { onClose: vi.fn(), onNavigateToWorkflow: vi.fn(), onViewAllWorkflows: vi.fn() };
    const view = render(<I18nProvider><ActiveRunsPopover {...props} workflows={[workflow]} /></I18nProvider>);
    screen.getByRole('button', { name: 'Arrêter' }).focus();
    view.rerender(<I18nProvider><ActiveRunsPopover {...props} workflows={[]} /></I18nProvider>);
    await act(async () => {});
    expect(screen.getByRole('dialog')).toHaveFocus();
    fireEvent.keyDown(document.activeElement ?? document, { key: 'Escape' });
    expect(props.onClose).toHaveBeenCalledTimes(1);
  });
});
