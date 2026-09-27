import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { Workflow } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join(',')}` : key,
    locale: 'en',
    setLocale: () => {},
  }),
}));

import { workflows as workflowsApi } from '../../../lib/api';
import { ChildWorkflowVariablesEditor } from '../ChildWorkflowVariablesEditor';

const phase3 = {
  id: 'phase-3',
  name: 'Phase 3',
  variables: [
    { name: 'ticketKey', label: 'Ticket', placeholder: '', required: true, allow_manual_override: false },
    { name: 'token', label: 'Token', placeholder: '', required: true, source: 'project_env', source_ref: '<env.TOKEN>', allow_manual_override: false },
  ],
} as unknown as Workflow;

describe('ChildWorkflowVariablesEditor (KT-796)', () => {
  beforeEach(() => vi.mocked(workflowsApi.get).mockReset());

  it('lists the target workflow\'s launch variables and maps a template to each input', async () => {
    vi.mocked(workflowsApi.get).mockResolvedValue(phase3);
    const onChange = vi.fn();
    render(<ChildWorkflowVariablesEditor targetId="phase-3" value={{}} onChange={onChange} />);

    const ticket = await screen.findByLabelText('wiz.childVariableFor:ticketKey');
    expect(workflowsApi.get).toHaveBeenCalledWith('phase-3');
    // A secret resolved by the child is shown, never asked for.
    expect(screen.queryByLabelText('wiz.childVariableFor:token')).not.toBeInTheDocument();
    expect(screen.getByText('wiz.childVariableResolvedByChild')).toBeInTheDocument();

    fireEvent.change(ticket, { target: { value: '{{ticketKey}}' } });
    expect(onChange).toHaveBeenCalledWith({ ticketKey: '{{ticketKey}}' });
  });

  it('flags a mapped name the target does not declare and lets it be removed', async () => {
    vi.mocked(workflowsApi.get).mockResolvedValue(phase3);
    const onChange = vi.fn();
    render(
      <ChildWorkflowVariablesEditor
        targetId="phase-3"
        value={{ ticketKey: '{{ticket}}', legacy: 'x' }}
        onChange={onChange}
      />,
    );

    expect(await screen.findByText('wiz.childVariablesUndeclared')).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText('wiz.childVariableRemove:legacy'));
    expect(onChange).toHaveBeenCalledWith({ ticketKey: '{{ticket}}' });
  });

  it('renders nothing without a target, and reads nothing for a bundle placeholder', () => {
    const { container } = render(
      <ChildWorkflowVariablesEditor targetId={null} value={{}} onChange={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
    render(<ChildWorkflowVariablesEditor targetId="@bundle:child" value={{}} onChange={vi.fn()} />);
    expect(workflowsApi.get).not.toHaveBeenCalled();
  });
});
