import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react';

// I18nProvider's sync effect calls config.getUiLanguage(); mock it so the
// effect resolves cleanly. SecretField itself uses no API.
vi.mock('../../lib/api', () => ({
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
  },
}));

import { I18nProvider } from '../../lib/I18nContext';
import { SecretField } from '../SecretField';

afterEach(cleanup);
const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('SecretField (refonte 2026-06-09)', () => {
  it('create mode: editable masked input, eye toggles visibility', () => {
    const onChange = vi.fn();
    wrap(<SecretField value="" onChange={onChange} />);
    const input = screen.getByPlaceholderText('Valeur') as HTMLInputElement;
    expect(input.type).toBe('password');
    fireEvent.change(input, { target: { value: 'abc' } });
    expect(onChange).toHaveBeenCalledWith('abc');
    // Eye reveals the typed value.
    fireEvent.click(screen.getByLabelText('Afficher'));
    expect((screen.getByPlaceholderText('Valeur') as HTMLInputElement).type).toBe('text');
  });

  it('stored mode: masked + Remplacer, no editable input; eye peeks via onRevealStored', async () => {
    const onReplace = vi.fn();
    const onRevealStored = vi.fn().mockResolvedValue('sk-REAL-123');
    wrap(
      <SecretField value="" onChange={() => {}} stored onReplace={onReplace} onRevealStored={onRevealStored} />,
    );
    // No editable value input in stored mode; a "Remplacer" link is shown.
    expect(screen.queryByPlaceholderText('Valeur')).toBeNull();
    expect(screen.getByText('Remplacer')).toBeTruthy();

    // Eye peeks the stored value read-only (fetched on demand).
    fireEvent.click(screen.getByLabelText('Afficher'));
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(onRevealStored).toHaveBeenCalledTimes(1);
    const revealed = screen.getByDisplayValue('sk-REAL-123') as HTMLInputElement;
    expect(revealed.readOnly).toBe(true);

    // "Remplacer" delegates to the parent.
    fireEvent.click(screen.getByText('Remplacer'));
    expect(onReplace).toHaveBeenCalledTimes(1);
  });

  it('replacing mode: empty editable input + Annuler', () => {
    const onCancel = vi.fn();
    wrap(<SecretField value="" onChange={() => {}} stored replacing onCancelReplace={onCancel} />);
    const input = screen.getByPlaceholderText('Valeur') as HTMLInputElement;
    expect(input.value).toBe('');
    fireEvent.click(screen.getByText('Annuler'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
