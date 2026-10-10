import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ origins: vi.fn(), change: vi.fn() }));
vi.mock('../../../lib/api', () => ({ config: { getEmbedOrigins: mocks.origins, changeEmbedOrigins: mocks.change } }));
vi.mock('../../../hooks/useWebSocket', () => ({ useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })) }));
vi.mock('../../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: unknown[]) => args.length ? `${key}:${args.join(',')}` : key }) }));

import { ExternalContentSection, EXTERNAL_CONTENT_SECTION_ID } from '../ExternalContentSection';
import { resetEmbedAllowedOriginsForTests } from '../../../hooks/useEmbedAllowedOrigins';

const toast = vi.fn();
const field = () => screen.getByLabelText('settings.embeds.originLabel') as HTMLInputElement;
const addButton = () => screen.getByRole('button', { name: /common\.add/ });
const listed = () => Array.from(document.querySelectorAll<HTMLElement>('[data-testid="settings-embed-origins"] [data-origin]'))
  .map(row => row.dataset.origin);

/** A backend that keeps its list, so a remount reads what was saved. */
let stored: string[] = [];
beforeEach(() => {
  resetEmbedAllowedOriginsForTests();
  stored = [];
  vi.clearAllMocks();
  mocks.origins.mockImplementation(async () => [...stored]);
  mocks.change.mockImplementation(async ({ add, remove }: { add: string[]; remove: string[] }) => {
    stored = [...stored.filter(origin => !remove.includes(origin)), ...add.filter(origin => !stored.includes(origin))];
    return [...stored];
  });
  toast.mockReset();
});
afterEach(cleanup);

describe('ExternalContentSection', () => {
  it('says when no site is allowed and explains the scope of a permission', async () => {
    render(<ExternalContentSection toast={toast} />);
    expect(await screen.findByText('settings.embeds.empty')).toBeInTheDocument();
    expect(screen.getByText('settings.embeds.explanation')).toBeInTheDocument();
    expect(screen.getByText('settings.embeds.subtitle')).toBeInTheDocument();
    expect(document.getElementById(EXTERNAL_CONTENT_SECTION_ID)).not.toBeNull();
  });

  it('shows the exact origin it will save, adds it, and keeps it after a reload', async () => {
    const view = render(<ExternalContentSection toast={toast} />);
    await screen.findByText('settings.embeds.empty');
    fireEvent.change(field(), { target: { value: ' https://Player.Example.com/ ' } });
    expect(screen.getByTestId('settings-embed-origin-status')).toHaveTextContent('settings.embeds.willSave https://player.example.com');
    fireEvent.click(addButton());
    await waitFor(() => expect(listed()).toEqual(['https://player.example.com']));
    expect(mocks.change).toHaveBeenCalledWith({ add: ['https://player.example.com'], remove: [] });
    expect(field().value).toBe('');

    view.unmount();
    resetEmbedAllowedOriginsForTests();
    render(<ExternalContentSection toast={toast} />);
    await waitFor(() => expect(listed()).toEqual(['https://player.example.com']));
  });

  it('refuses a malformed origin and a duplicate before calling the backend', async () => {
    stored = ['https://suno.com'];
    render(<ExternalContentSection toast={toast} />);
    await waitFor(() => expect(listed()).toEqual(['https://suno.com']));
    fireEvent.change(field(), { target: { value: 'https://suno.com/embed/abc' } });
    expect(screen.getByTestId('settings-embed-origin-status')).toHaveTextContent('settings.embeds.invalid');
    expect(field()).toHaveAttribute('aria-invalid', 'true');
    expect(addButton()).toBeDisabled();
    fireEvent.change(field(), { target: { value: 'https://SUNO.com' } });
    expect(screen.getByTestId('settings-embed-origin-status')).toHaveTextContent('settings.embeds.duplicate');
    expect(addButton()).toBeDisabled();
    fireEvent.keyDown(field(), { key: 'Enter' });
    expect(mocks.change).not.toHaveBeenCalled();
  });

  it('revokes a site', async () => {
    stored = ['https://suno.com', 'https://player.example.com'];
    render(<ExternalContentSection toast={toast} />);
    await waitFor(() => expect(listed()).toHaveLength(2));
    const row = document.querySelector('[data-origin="https://suno.com"]')!;
    fireEvent.click(row.querySelector('button')!);
    await waitFor(() => expect(listed()).toEqual(['https://player.example.com']));
    expect(mocks.change).toHaveBeenCalledWith({ add: [], remove: ['https://suno.com'] });
  });

  it('types a prefilled site in, scrolls to it and focuses it, without adding it', async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    const view = render(<ExternalContentSection toast={toast} prefill={{ origin: 'https://vimeo.com', nonce: 1 }} />);
    await waitFor(() => expect(field().value).toBe('https://vimeo.com'));
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    await waitFor(() => expect(document.activeElement).toBe(field()));
    expect(mocks.change).not.toHaveBeenCalled();
    // Asking again for the same site refocuses it.
    fireEvent.change(field(), { target: { value: '' } });
    view.rerender(<ExternalContentSection toast={toast} prefill={{ origin: 'https://vimeo.com', nonce: 2 }} />);
    await waitFor(() => expect(field().value).toBe('https://vimeo.com'));
  });

  it('reports a failed save', async () => {
    mocks.change.mockRejectedValueOnce(new Error('disk full'));
    render(<ExternalContentSection toast={toast} />);
    await screen.findByText('settings.embeds.empty');
    fireEvent.change(field(), { target: { value: 'https://player.example.com' } });
    await act(async () => { fireEvent.click(addButton()); });
    await waitFor(() => expect(toast).toHaveBeenCalledWith(expect.stringContaining('common.actionFailed'), 'error'));
    expect(field().value).toBe('https://player.example.com');
  });
});
