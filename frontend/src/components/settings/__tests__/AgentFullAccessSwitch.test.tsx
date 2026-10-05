import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, act } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import { AgentFullAccessSwitch } from '../AgentFullAccessSwitch';

afterEach(cleanup);

const setup = (checked: boolean, onChange: (next: boolean) => Promise<void>) =>
  render(
    <I18nProvider>
      <AgentFullAccessSwitch agentName="Claude Code" checked={checked} onChange={onChange} />
    </I18nProvider>,
  );

describe('AgentFullAccessSwitch', () => {
  it('is a labelled native switch', () => {
    setup(false, vi.fn().mockResolvedValue(undefined));
    const sw = screen.getByRole('switch', { name: /Claude Code/ });
    expect(sw.tagName).toBe('BUTTON');
    expect(sw.getAttribute('aria-checked')).toBe('false');
  });

  it('explains the risk before enabling and does nothing on Escape', () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    setup(false, onChange);
    fireEvent.click(screen.getByRole('switch'));
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.textContent).toMatch(/injection/i);
    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('applies a confirmed enable', async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    setup(false, onChange);
    fireEvent.click(screen.getByRole('switch'));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^(Activer l'accès complet|Enable full access)$/ }));
    });
    expect(onChange).toHaveBeenCalledExactlyOnceWith(true);
  });

  it('blocks a double submit of a disable while the first is in flight', async () => {
    let release: (() => void) | undefined;
    const onChange = vi.fn(() => new Promise<void>(r => { release = r; }));
    setup(true, onChange);
    const sw = screen.getByRole('switch');
    await act(async () => {
      fireEvent.click(sw);
      fireEvent.click(sw);
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    await act(async () => { release?.(); });
  });

  it('disables without a confirmation', async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    setup(true, onChange);
    await act(async () => { fireEvent.click(screen.getByRole('switch')); });
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it('shows a locked, always-on state that cannot be toggled', () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    render(
      <I18nProvider>
        <AgentFullAccessSwitch agentName="Codex" checked={false} locked onChange={onChange} />
      </I18nProvider>,
    );
    const sw = screen.getByRole('switch');
    expect(sw.getAttribute('aria-checked')).toBe('true');
    expect(sw).toBeDisabled();
    expect(sw.textContent).toMatch(/Codex/);
    fireEvent.click(sw);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });
});
