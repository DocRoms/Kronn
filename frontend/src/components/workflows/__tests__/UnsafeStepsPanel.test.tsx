import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { UnsafeExecStep, Workflow } from '../../../types/generated';

const unsafeSteps = vi.fn();
vi.mock('../../../lib/api', () => ({
  workflows: { unsafeSteps: (...args: unknown[]) => unsafeSteps(...args) },
}));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { UnsafeStepsPanel } from '../UnsafeStepsPanel';

const workflow = { id: 'wf-1' } as Workflow;
const fixable: UnsafeExecStep = {
  step_name: 'greet',
  on_failure: false,
  phase: 'main',
  command: 'bash',
  args: ['-c', 'echo {{issue.title}}'],
  placeholder: '{{issue.title}}',
  reason: 'inline_code_interpolation',
  suggested_args: ['-c', 'echo "$1"', '_', '{{issue.title}}'],
  manual_fix: null,
};
const manual: UnsafeExecStep = {
  ...fixable,
  step_name: 'heredoc',
  args: ['-c', 'cat <<EOF\n{{issue.title}}\nEOF'],
  suggested_args: null,
  manual_fix: 'correction manuelle requise : le script contient un heredoc',
};

describe('UnsafeStepsPanel', () => {
  beforeEach(() => unsafeSteps.mockReset());

  it('renders nothing when every step is safe', async () => {
    unsafeSteps.mockResolvedValue([]);
    const { container } = render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await waitFor(() => expect(unsafeSteps).toHaveBeenCalledWith('wf-1'));
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the diff on request and applies only on click', async () => {
    unsafeSteps.mockResolvedValue([fixable]);
    const onApply = vi.fn().mockResolvedValue(undefined);
    render(<UnsafeStepsPanel workflow={workflow} onApply={onApply} />);

    await screen.findByText('{{issue.title}}');
    expect(screen.queryByText('wf.unsafeApply')).toBeNull();
    fireEvent.click(screen.getByText('wf.unsafeSuggest'));
    expect(screen.getByText(/\+ bash \["-c","echo \\"\$1\\"","_","\{\{issue.title\}\}"\]/)).toBeDefined();
    expect(onApply).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText('wf.unsafeApply'));
    await waitFor(() => expect(onApply).toHaveBeenCalledWith(fixable));
  });

  it('offers no automatic rewrite when the fix is manual', async () => {
    unsafeSteps.mockResolvedValue([manual]);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await screen.findByText('heredoc');
    fireEvent.click(screen.getByText('wf.unsafeSuggest'));
    expect(screen.getByText('wf.unsafeManual')).toBeDefined();
    expect(screen.getByText(manual.manual_fix!)).toBeDefined();
    expect(screen.queryByText('wf.unsafeApply')).toBeNull();
  });

  it('names the CollectApiData source of an inline Quick Exec', async () => {
    unsafeSteps.mockResolvedValue([{ ...fixable, step_name: 'collect', phase: 'source', source_alias: 'ticket' }]);
    const onApply = vi.fn().mockResolvedValue(undefined);
    render(<UnsafeStepsPanel workflow={workflow} onApply={onApply} />);
    await screen.findByText('collect');
    expect(screen.getByText(/wf\.unsafeSource/)).toBeDefined();
    fireEvent.click(screen.getByText('wf.unsafeSuggest'));
    fireEvent.click(screen.getByText('wf.unsafeApply'));
    await waitFor(() => expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ source_alias: 'ticket' })));
  });
});
