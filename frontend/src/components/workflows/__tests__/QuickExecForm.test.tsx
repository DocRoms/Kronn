import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { QuickExecForm } from '../QuickExecForm';

afterEach(cleanup);

describe('QuickExecForm', () => {
  it('builds literal argv, CSV output and declared variables', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);
    const { container } = render(
      <QuickExecForm
        projects={[{ id: 'project-1', name: 'Kronn' }] as Parameters<typeof QuickExecForm>[0]['projects']}
        onSave={onSave}
        onCancel={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText('qe.name *'), { target: { value: 'AWS inventory' } });
    fireEvent.change(screen.getByLabelText('qe.command *'), { target: { value: 'aws' } });
    fireEvent.change(container.querySelector('textarea.qe-args')!, {
      target: { value: 'ec2\ndescribe-instances\n--region\n{{region}}\n--output\ntext' },
    });
    fireEvent.change(screen.getByLabelText('qe.project'), { target: { value: 'project-1' } });
    fireEvent.change(screen.getByLabelText('qe.outputFormat'), { target: { value: 'csv' } });
    fireEvent.click(screen.getByText('qe.addVariable'));
    fireEvent.change(screen.getByLabelText('qe.variableName'), { target: { value: 'region' } });
    fireEvent.change(screen.getByLabelText('qe.variableLabel'), { target: { value: 'AWS region' } });
    fireEvent.click(screen.getByText('qe.save'));

    await waitFor(() => expect(onSave).toHaveBeenCalledOnce());
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      name: 'AWS inventory',
      project_id: 'project-1',
      command: 'aws',
      args: ['ec2', 'describe-instances', '--region', '{{region}}', '--output', 'text'],
      output_format: 'csv',
      variables: [expect.objectContaining({ name: 'region', label: 'AWS region', required: true })],
    }));
  });
});
