import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());

import { DecisionModelPicker } from '../DecisionModelPicker';
import type { DecisionModelOption } from '../../lib/decisionModels';

const local = (model: string, extra: Partial<DecisionModelOption> = {}): DecisionModelOption => ({
  model, connectionId: null, label: model, transport: 'local', unavailable: false, ...extra,
});
const DEFAULT = { model: null, connectionId: null };

describe('DecisionModelPicker (KT-1697)', () => {
  it('shows « Décision · model » with its transport badge, the default first in the menu', async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    render(<DecisionModelPicker choice={DEFAULT} defaultModel="clef-flash:9b" ariaLabel="pick"
      options={[local('clef-flash:9b'), local('clef-pro:27b')]} onChange={onChange} />);
    const button = screen.getByTestId('decision-model-picker');
    expect(button).toHaveTextContent('decisionPicker.name');
    expect(screen.getByTestId('decision-model-name')).toHaveTextContent('clef-flash:9b');
    expect(screen.getByTestId('decision-transport')).toHaveTextContent('decisionPicker.transport.local');

    fireEvent.click(button);
    const items = await screen.findAllByRole('menuitem');
    expect(items[0]).toHaveAttribute('data-testid', 'decision-option-default');
    expect(items[0]).toBeDisabled(); // already the choice
    expect(items.map(item => item.getAttribute('data-testid'))).toEqual([
      'decision-option-default', 'decision-option-clef-flash:9b', 'decision-option-clef-pro:27b',
    ]);

    fireEvent.click(screen.getByTestId('decision-option-clef-pro:27b'));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ model: 'clef-pro:27b', connectionId: null }));
  });

  it('keeps the menu open when the save fails, so another model can be picked', async () => {
    const onChange = vi.fn().mockRejectedValue(new Error('refused'));
    render(<DecisionModelPicker choice={DEFAULT} defaultModel="a" ariaLabel="pick"
      options={[local('a'), local('b')]} onChange={onChange} />);
    fireEvent.click(screen.getByTestId('decision-model-picker'));
    fireEvent.click(await screen.findByTestId('decision-option-b'));
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByTestId('decision-option-b')).not.toBeDisabled());
    expect(screen.getAllByRole('menuitem').length).toBeGreaterThan(0);
  });

  it('goes back to the configured model from « Par défaut »', async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    render(<DecisionModelPicker choice={{ model: 'b', connectionId: null }} defaultModel="a" ariaLabel="pick"
      options={[local('a'), local('b')]} onChange={onChange} />);
    expect(screen.getByTestId('decision-model-name')).toHaveTextContent('b');
    fireEvent.click(screen.getByTestId('decision-model-picker'));
    fireEvent.click(await screen.findByTestId('decision-option-default'));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(DEFAULT));
  });

  it('with a single decision model, still shows it, read-only', () => {
    render(<DecisionModelPicker choice={DEFAULT} defaultModel="clef-flash:9b" ariaLabel="pick"
      options={[local('clef-flash:9b')]} onChange={vi.fn()} />);
    expect(screen.queryByTestId('decision-model-picker')).toBeNull();
    expect(screen.getByTestId('decision-model-static')).toHaveTextContent('clef-flash:9b');
    expect(screen.getByTestId('decision-transport')).toBeInTheDocument();
  });

  it('flags a chosen model the catalogue does not list, never swapping it', async () => {
    render(<DecisionModelPicker choice={{ model: 'ghost:7b', connectionId: null }} defaultModel="a" ariaLabel="pick"
      options={[local('a'), local('b')]} onChange={vi.fn()} />);
    expect(screen.getByTestId('decision-model-name')).toHaveTextContent('ghost:7b');
    expect(screen.getByRole('img', { name: 'decisionPicker.missing' })).toBeInTheDocument();
  });

  it('badges a connection\'s model as HTTP', () => {
    render(<DecisionModelPicker choice={{ model: 'pplx-decider', connectionId: 'conn-1' }} defaultModel="a"
      ariaLabel="pick" options={[local('a'), local('b')]} onChange={vi.fn()} />);
    expect(screen.getByTestId('decision-transport')).toHaveTextContent('decisionPicker.transport.http');
  });
});
