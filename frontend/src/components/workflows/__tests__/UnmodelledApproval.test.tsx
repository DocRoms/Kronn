import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const execLineCheck = vi.fn();
vi.mock('../../../lib/api', () => ({
  workflows: { execLineCheck: (...args: unknown[]) => execLineCheck(...args) },
}));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, arg?: string) => `${key}:${arg ?? ''}` }),
}));

import { UnmodelledApproval } from '../UnmodelledApproval';

describe('UnmodelledApproval', () => {
  beforeEach(() => execLineCheck.mockReset());

  it('stays hidden when the line needs no approval', async () => {
    execLineCheck.mockResolvedValue({ unmodelled_program: null, covered: [] });
    const { container } = render(
      <UnmodelledApproval command="echo" args={['{{x}}']} approved={false} onChange={vi.fn()} />,
    );
    await waitFor(() => expect(execLineCheck).toHaveBeenCalledWith({ command: 'echo', args: ['{{x}}'] }));
    expect(container).toBeEmptyDOMElement();
  });

  it('names the program and reports the human approval', async () => {
    execLineCheck.mockResolvedValue({ unmodelled_program: 'terraform', covered: ['main: terraform'] });
    const onChange = vi.fn();
    render(<UnmodelledApproval command="terraform" args={['plan', '{{x}}']} approved={false} onChange={onChange} />);
    const box = await screen.findByRole('checkbox');
    expect(screen.getByText('exec.unmodelledApprove:terraform')).toBeDefined();
    fireEvent.click(box);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it('lists every line the approval covers and sends the setup line and the writer', async () => {
    execLineCheck.mockResolvedValue({
      unmodelled_program: 'aws',
      covered: ['main: aws', 'setup: terraform'],
    });
    render(
      <UnmodelledApproval
        command="aws" args={['s3', 'ls', '{{b}}']}
        setupCommand="terraform" setupArgs={['init', '{{x}}']} agentWritten
        approved={false} onChange={vi.fn()}
      />,
    );
    await waitFor(() => expect(execLineCheck).toHaveBeenCalledWith({
      command: 'aws', args: ['s3', 'ls', '{{b}}'],
      setup_command: 'terraform', setup_args: ['init', '{{x}}'], agent_written: true,
    }));
    expect(await screen.findByText('exec.unmodelledApprove:aws, terraform')).toBeDefined();
    expect(screen.getByText('exec.unmodelledCovers:main: aws · setup: terraform')).toBeDefined();
  });

  it('drops a stored approval once no line needs it', async () => {
    execLineCheck.mockResolvedValue({ unmodelled_program: null, covered: [] });
    const onChange = vi.fn();
    render(<UnmodelledApproval command="echo" args={['{{x}}']} approved onChange={onChange} />);
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(false));
  });

  it('sends the stdin template so a stdin-only line can be approved', async () => {
    execLineCheck.mockResolvedValue({ unmodelled_program: 'duckdb', covered: ['stdin: duckdb'] });
    render(
      <UnmodelledApproval command="duckdb" args={['db.duckdb']} stdin="{{issue.title}}" approved={false} onChange={vi.fn()} />,
    );
    await waitFor(() => expect(execLineCheck).toHaveBeenCalledWith({
      command: 'duckdb', args: ['db.duckdb'], stdin: '{{issue.title}}',
    }));
    expect(await screen.findByText('exec.unmodelledApprove:duckdb')).toBeDefined();
  });
});
