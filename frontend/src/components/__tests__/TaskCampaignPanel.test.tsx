import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { TaskCampaignPanel } from '../TaskCampaignPanel';
import type { CampaignView, CampaignTaskCandidate } from '../../lib/api';

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const candidate = (over: Partial<CampaignTaskCandidate> & { reference: string }): CampaignTaskCandidate => ({
  task: { reference: over.reference, title: `Task ${over.reference}`, status: 'todo' } as CampaignTaskCandidate['task'],
  plan_position: 0,
  launchable: true,
  reasons: [],
  ...over,
});

const view = (over: Partial<CampaignView> = {}): CampaignView => ({
  run: { id: 'run-1', status: 'active' } as CampaignView['run'],
  candidates: [],
  principal_attention: {
    active_executions: 0,
    cli_executions: 0,
    awaiting_review: 0,
    awaiting_human: 0,
    ready_tasks: 0,
    actions: [],
  },
  ...over,
});

describe('TaskCampaignPanel', () => {
  beforeEach(() => localStorage.clear());

  it('stays out of the way when nothing runs and nothing can start', () => {
    // An empty section is a box asking to be understood. It earns its place only
    // once there is a decision to take or work to watch.
    wrap(<TaskCampaignPanel view={view()} onLaunch={vi.fn()} />);
    expect(screen.queryByTestId('orch-campaign')).toBeNull();
  });

  it('renders nothing at all before the campaign is loaded', () => {
    wrap(<TaskCampaignPanel view={null} onLaunch={vi.fn()} />);
    expect(screen.queryByTestId('orch-campaign')).toBeNull();
  });

  it('offers the launch only on a task that is really ready', () => {
    const onLaunch = vi.fn();
    wrap(
      <TaskCampaignPanel
        view={view({ candidates: [candidate({ reference: 'KT-1' })] })}
        onLaunch={onLaunch}
      />
    );

    fireEvent.click(screen.getByTestId('orch-launch-KT-1'));
    expect(onLaunch).toHaveBeenCalledWith('KT-1');
  });

  it('replaces the CTA with its reason when a task is not launchable', () => {
    // Not a disabled button: a greyed CTA still reads as "this is the way", and
    // the way is elsewhere until the reason is dealt with.
    wrap(
      <TaskCampaignPanel
        view={view({
          candidates: [
            candidate({
              reference: 'KT-2',
              launchable: false,
              reasons: [{ code: 'blocked', detail: 'KT-9 doit être terminée avant' }],
            }),
          ],
          principal_attention: { ...view().principal_attention, active_executions: 1 },
        })}
        onLaunch={vi.fn()}
      />
    );

    expect(screen.queryByTestId('orch-launch-KT-2')).toBeNull();
    expect(screen.getByTestId('orch-reasons-KT-2').textContent).toContain('KT-9 doit être terminée avant');
  });

  it('says what the principal owes, so the coordinator is not an opaque badge', () => {
    wrap(
      <TaskCampaignPanel
        view={view({
          candidates: [candidate({ reference: 'KT-3' })],
          principal_attention: {
            ...view().principal_attention,
            active_executions: 2,
            awaiting_review: 1,
            awaiting_human: 3,
          },
        })}
        onLaunch={vi.fn()}
      />
    );

    const line = screen.getByTestId('orch-campaign-attention').textContent ?? '';
    expect(line).toContain('2');
    expect(line).toContain('1');
    expect(line).toContain('3');
  });

  it('remembers that this campaign was folded away, across a remount', () => {
    // DoD-7: a reload that reopens what the user deliberately folded is the same
    // annoyance as one that folds away what they were watching.
    const v = view({ candidates: [candidate({ reference: 'KT-5' })] });
    const first = wrap(<TaskCampaignPanel view={v} onLaunch={vi.fn()} />);
    expect(screen.getByTestId('orch-campaign-toggle').getAttribute('aria-expanded')).toBe('true');

    fireEvent.click(screen.getByTestId('orch-campaign-toggle'));
    expect(screen.queryByTestId('orch-launch-KT-5')).toBeNull();
    first.unmount();

    wrap(<TaskCampaignPanel view={v} onLaunch={vi.fn()} />);
    expect(screen.getByTestId('orch-campaign-toggle').getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByTestId('orch-launch-KT-5')).toBeNull();
  });

  it('restores folding when the campaign arrives after the panel mounted', async () => {
    localStorage.setItem('run-late:orchCollapsed', '1');
    const late = view({
      run: { id: 'run-late', status: 'active' } as CampaignView['run'],
      candidates: [candidate({ reference: 'KT-9' })],
    });
    const rendered = wrap(<TaskCampaignPanel view={null} onLaunch={vi.fn()} />);
    rendered.rerender(<I18nProvider><TaskCampaignPanel view={late} onLaunch={vi.fn()} /></I18nProvider>);
    await waitFor(() => {
      expect(screen.getByTestId('orch-campaign-toggle')).toHaveAttribute('aria-expanded', 'false');
    });
  });

  it('keeps the answer per campaign rather than globally', () => {
    // Folding one campaign away must not decide for another: the question was
    // about this run, not about campaigns in general.
    const folded = view({ run: { id: 'run-A', status: 'active' } as CampaignView['run'], candidates: [candidate({ reference: 'KT-6' })] });
    const other = view({ run: { id: 'run-B', status: 'active' } as CampaignView['run'], candidates: [candidate({ reference: 'KT-7' })] });

    const first = wrap(<TaskCampaignPanel view={folded} onLaunch={vi.fn()} />);
    fireEvent.click(screen.getByTestId('orch-campaign-toggle'));
    first.unmount();

    wrap(<TaskCampaignPanel view={other} onLaunch={vi.fn()} />);
    expect(screen.getByTestId('orch-campaign-toggle').getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('orch-launch-KT-7')).toBeTruthy();
  });

  it('launches from the keyboard, not only from a pointer', () => {
    const onLaunch = vi.fn();
    wrap(
      <TaskCampaignPanel
        view={view({ candidates: [candidate({ reference: 'KT-8' })] })}
        onLaunch={onLaunch}
      />
    );
    const button = screen.getByTestId('orch-launch-KT-8');
    button.focus();
    expect(document.activeElement).toBe(button);
    fireEvent.keyDown(button, { key: 'Enter' });
    fireEvent.click(button); // what the browser dispatches for Enter on a button
    expect(onLaunch).toHaveBeenCalledWith('KT-8');
  });

  it('does not fire a second launch while one is starting', () => {
    const onLaunch = vi.fn();
    wrap(
      <TaskCampaignPanel
        view={view({ candidates: [candidate({ reference: 'KT-4' })] })}
        onLaunch={onLaunch}
        busyTaskReference="KT-4"
      />
    );

    fireEvent.click(screen.getByTestId('orch-launch-KT-4'));
    expect(onLaunch).not.toHaveBeenCalled();
  });
});
