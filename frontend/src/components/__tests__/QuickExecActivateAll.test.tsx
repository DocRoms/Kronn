/**
 * KT-1695 — « Activate all (N) »: one confirmation for every Quick Exec that
 * waits, one checkbox per distinct program, every line activated through the
 * single human-only endpoint (the server refuses an agent).
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  activate: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  quickExecs: { activate: mocks.activate },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));

import { PendingActivationNotice } from '../QuickExecActivation';

const logs = {
  id: 'qe-2', name: 'Top 5xx', program: 'aws', command: 'aws',
  args: ['logs', 'get-query-results', '--query-id', '{{query_id}}'], steps: ['top-a', 'top-b', 'top-c'],
};
const start = {
  id: 'qe-3', name: 'Start query', program: 'aws', command: 'aws',
  args: ['logs', 'start-query', '--start-time', '{{from}}'], steps: ['cw-start'],
};
const cdn = {
  id: 'qe-1', name: 'Purge CDN', program: 'fastly', command: 'fastly',
  args: ['purge', '--key', '{{surrogate_key}}'], steps: ['purge'],
};

const open = () => fireEvent.click(screen.getByText('qeActivateAll.button:2'));
const confirmButton = (count: number) =>
  screen.getByText(`qeActivateAll.confirm:${count}`) as HTMLButtonElement;

beforeEach(() => {
  mocks.activate.mockReset();
  mocks.activate.mockResolvedValue({});
});

describe('activate all', () => {
  it('is offered from two waiting Quick Execs on, and not for one; the per-card button stays', () => {
    const { rerender } = render(<PendingActivationNotice items={[logs]} />);
    expect(screen.queryByText(/qeActivateAll.button/)).toBeNull();
    rerender(<PendingActivationNotice items={[logs, start]} />);
    expect(screen.getByText('qeActivateAll.button:2')).toBeDefined();
    expect(screen.getAllByText(/qeActivation.activateNamed/)).toHaveLength(2);
  });

  it('lists the steps that use each Quick Exec', () => {
    render(<PendingActivationNotice items={[logs, start]} />);
    expect(screen.getByText(/qeActivation.usedBy:top-a, top-b, top-c/)).toBeDefined();
  });

  it('opens one confirmation: every command collapsed, one checkbox for one program', async () => {
    const onActivated = vi.fn();
    render(<PendingActivationNotice items={[logs, start]} onActivated={onActivated} />);
    expect(screen.queryByTestId('qe-activate-all-confirm')).toBeNull();
    open();
    const confirmation = screen.getByTestId('qe-activate-all-confirm');
    const details = confirmation.querySelectorAll('details');
    expect(details).toHaveLength(2);
    details.forEach(item => expect(item.hasAttribute('open')).toBe(false));
    expect(confirmation.textContent).toContain('aws logs get-query-results --query-id {{query_id}}');
    expect(confirmation.textContent).toContain('aws logs start-query --start-time {{from}}');
    // Two Quick Execs, one program: a single checkbox, with the existing wording.
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(1);
    expect(screen.getByText('exec.unmodelledApprove:aws')).toBeDefined();

    // Nothing is activated before the box is ticked and the confirmation clicked.
    expect(confirmButton(2).disabled).toBe(true);
    fireEvent.click(confirmButton(2));
    expect(mocks.activate).not.toHaveBeenCalled();
    fireEvent.click(boxes[0]);
    expect(confirmButton(2).disabled).toBe(false);
    fireEvent.click(confirmButton(2));
    await waitFor(() => expect(onActivated).toHaveBeenCalledTimes(1));
    expect(mocks.activate).toHaveBeenCalledTimes(2);
    expect(mocks.activate).toHaveBeenCalledWith('qe-2', { command: 'aws', args: logs.args });
    expect(mocks.activate).toHaveBeenCalledWith('qe-3', { command: 'aws', args: start.args });
  });

  it('asks one checkbox per distinct program and waits for every one', () => {
    render(<PendingActivationNotice items={[logs, start, cdn]} />);
    fireEvent.click(screen.getByText('qeActivateAll.button:3'));
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    expect(screen.getByText('exec.unmodelledApprove:aws')).toBeDefined();
    expect(screen.getByText('exec.unmodelledApprove:fastly')).toBeDefined();
    fireEvent.click(boxes[0]);
    expect(confirmButton(3).disabled).toBe(true);
    fireEvent.click(boxes[1]);
    expect(confirmButton(3).disabled).toBe(false);
  });

  it('cancels without activating anything', () => {
    render(<PendingActivationNotice items={[logs, start]} />);
    open();
    fireEvent.click(screen.getByText('qeActivateAll.cancel'));
    expect(screen.queryByTestId('qe-activate-all-confirm')).toBeNull();
    expect(mocks.activate).not.toHaveBeenCalled();
  });

  it('activates what it can, names the line the server refused and says so', async () => {
    mocks.activate.mockImplementation(async (id: string) => {
      if (id === 'qe-3') throw new Error('La ligne a changé');
      return {};
    });
    const onActivated = vi.fn();
    render(<PendingActivationNotice items={[logs, start]} onActivated={onActivated} />);
    open();
    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(confirmButton(2));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Start query');
    expect(alert.textContent).toContain('a changé');
    expect(mocks.activate).toHaveBeenCalledTimes(2);
    expect(onActivated).toHaveBeenCalledTimes(1);
  });

  it('is refused by the server for an agent: nothing is reported as activated', async () => {
    mocks.activate.mockRejectedValue(new Error('Only a human activates a Quick Exec: an agent cannot'));
    const onActivated = vi.fn();
    const onRefresh = vi.fn();
    render(<PendingActivationNotice items={[logs, start]} onActivated={onActivated} onRefresh={onRefresh} />);
    open();
    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(confirmButton(2));
    await waitFor(() => expect(screen.getAllByRole('alert')).toHaveLength(2));
    expect(onActivated).not.toHaveBeenCalled();
    expect(onRefresh).toHaveBeenCalledTimes(1);
  });
});
