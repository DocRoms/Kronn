// The comparative Quick Prompt's selector, shared with the TD talk (KT-982).

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/react';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));

import { AgentCompareSelector } from '../AgentCompareSelector';
import { defaultCompareTargets } from '../../lib/compareTargets';

const CHOICES = [{ agent: 'ClaudeCode' as const }, { agent: 'Custom' as const, connectionId: 'c1', label: 'Local' }];

function renderSelector(targets = defaultCompareTargets(CHOICES, 'default'), onChange = vi.fn()) {
  render(
    <AgentCompareSelector
      targets={targets}
      choices={CHOICES}
      unusable={[{ connectionId: 'c-off', label: 'Off', reason: 'no_endpoint' }]}
      installedAgentTypes={['ClaudeCode']}
      defaultTier="economy"
      addAgent="ClaudeCode"
      testIdPrefix="x"
      onChange={onChange}
    />,
  );
  return onChange;
}

describe('AgentCompareSelector', () => {
  it('removes a target, toggles everyone off and shows unusable connections', () => {
    const onChange = renderSelector();
    fireEvent.click(screen.getByTestId('x-chip-c1'));
    expect(onChange).toHaveBeenLastCalledWith([{ agent: 'ClaudeCode', tier: 'default' }]);
    fireEvent.click(screen.getByText('qp.compareAgents.selectNone'));
    expect(onChange).toHaveBeenLastCalledWith([]);
    expect(screen.getByTestId('x-chip-unusable-c-off')).toHaveTextContent('qp.compareAgents.unusable.no_endpoint');
  });

  it('adds a missing choice at the default tier and selects everyone again', () => {
    const onChange = renderSelector([{ agent: 'ClaudeCode', tier: 'default' }]);
    fireEvent.click(screen.getByTestId('x-chip-c1'));
    expect(onChange).toHaveBeenLastCalledWith([
      { agent: 'ClaudeCode', tier: 'default' },
      { agent: 'Custom', tier: 'economy', connection_id: 'c1' },
    ]);
    fireEvent.click(screen.getByText('qp.compareAgents.selectAll'));
    expect(onChange).toHaveBeenLastCalledWith(null);
    fireEvent.click(screen.getByText('qp.compareAgents.addModel'));
    expect(onChange).toHaveBeenLastCalledWith([
      { agent: 'ClaudeCode', tier: 'default' },
      { agent: 'ClaudeCode', tier: 'economy' },
    ]);
  });
});
