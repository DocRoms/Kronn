/** KT-255 — the session binding is provenance, shown read-only.
 *
 *  The removed form asked a human to pick from eight agents and type an opaque
 *  uuid. What is tested here is that the form is GONE from every angle a user
 *  could reach it, that an existing binding is still explained, and that the one
 *  repair action a stale binding needs survived the removal. */
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { discussions } from '../../lib/api';
import { DiscussionSessionBinding } from '../DiscussionSessionBinding';

vi.mock('../../lib/api', () => ({
  discussions: {
    sourceDetail: vi.fn(),
    sourceSessionStatus: vi.fn(),
    linkSourceSession: vi.fn(),
    unlinkSourceSession: vi.fn(),
  },
}));

const labels: Record<string, string> = {
  'disc.session.short': 'Session',
  'disc.session.title': 'Session CLI liée',
  'disc.session.contractVersion': 'Contrat v{0}',
  'disc.session.link': 'Lier une session',
  'disc.session.update': 'Mettre à jour',
  'disc.session.unlink': 'Délier',
  'disc.session.boundTooltip': '{0} · session {1}',
  'disc.session.connected': 'Connectée',
  'disc.session.offline': 'Hors ligne ou non détectée',
  'disc.session.copy': 'Copier {0}',
  'disc.session.agent': 'CLI source',
  'disc.session.id': 'ID de session',
  'disc.session.automatic': 'Liaison établie automatiquement au join de la CLI.',
  'disc.session.unlinked': 'Session déliée',
  'disc.session.unlinkFailed': 'Échec déliaison',
  'disc.session.history': 'Historique ({0})',
  'disc.session.closed': 'Terminée',
  'disc.session.current': 'Actuelle',
  'common.close': 'Fermer',
};

const t = (key: string, ...args: (string | number)[]) => {
  let value = labels[key] ?? key;
  args.forEach((arg, index) => {
    value = value.replace(`{${index}}`, String(arg));
  });
  return value;
};

const emptyDetail = { current: null, history: [] };

const bound = {
  current: {
    binding_version: 1,
    disc_id: 'disc-a',
    source_agent: 'ClaudeCode',
    source_session_id: 'claude-session-full-id',
    imported_at: null,
    diverged_at: null,
  },
  history: [],
};

describe('DiscussionSessionBinding', () => {
  const toast = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(discussions.sourceDetail).mockResolvedValue(emptyDetail);
    vi.mocked(discussions.sourceSessionStatus).mockResolvedValue({
      binding_version: 1,
      bound_disc_id: null,
      connected_disc_id: null,
      connection_status: null,
    });
    vi.mocked(discussions.linkSourceSession).mockResolvedValue(true);
    vi.mocked(discussions.unlinkSourceSession).mockResolvedValue(true);
  });

  // ── the form is gone ──────────────────────────────────────────────

  it('offers no control at all when no binding exists', async () => {
    // The old empty form sat here inviting a gesture that is almost always wrong.
    // Nothing to explain means nothing to show.
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    await waitFor(() => expect(discussions.sourceDetail).toHaveBeenCalledWith('disc-a'));
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('exposes no agent menu and no session-id field on an existing binding', async () => {
    // The two inputs the ticket names, checked by role rather than by label so a
    // renamed label cannot make this pass while the field is still there.
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));

    expect(await screen.findByRole('dialog')).toBeVisible();
    expect(screen.queryByRole('combobox')).toBeNull();
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('never calls the link endpoint from the UI', async () => {
    // The route stays for the bridge; the UI must have no path to it. This is the
    // assertion that would catch a form reintroduced under another name.
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));
    await screen.findByRole('dialog');

    screen
      .getAllByRole('button')
      .forEach(button => fireEvent.click(button));
    expect(discussions.linkSourceSession).not.toHaveBeenCalled();
  });

  // ── an existing binding is still explained ────────────────────────

  it('shows the bound agent, its state and the full id', async () => {
    // Provenance is information: which CLI this thread came from, and whether it
    // is still there.
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));

    expect(await screen.findByText('Hors ligne ou non détectée')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copier claude-session-full-id' })).toBeVisible();
  });

  it('says the binding was established automatically', async () => {
    // Otherwise a reader looks for the control that used to create one.
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));
    expect(await screen.findByText(/automatiquement/)).toBeVisible();
  });

  it('reports a live session as connected', async () => {
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    vi.mocked(discussions.sourceSessionStatus).mockResolvedValue({
      binding_version: 1,
      bound_disc_id: 'disc-a',
      connected_disc_id: 'disc-a',
      connection_status: 'connected',
    });
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));
    expect(await screen.findByText('Connectée')).toBeVisible();
  });

  // ── the repair path survived ──────────────────────────────────────

  it('can still unlink a stale binding', async () => {
    // DoD: no repair path lost without an alternative. This is the alternative —
    // there is no other way to clear a binding from the UI.
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Délier' }));

    await waitFor(() => expect(discussions.unlinkSourceSession).toHaveBeenCalledWith('disc-a'));
    expect(toast).toHaveBeenCalledWith('Session déliée', 'success');
  });

  it('emits the sidebar refresh event after unlinking', async () => {
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    const changed = vi.fn();
    window.addEventListener('kronn:disc-source-changed', changed);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Délier' }));

    await waitFor(() => expect(changed).toHaveBeenCalled());
    window.removeEventListener('kronn:disc-source-changed', changed);
  });

  it('reports a failed unlink instead of pretending it worked', async () => {
    vi.mocked(discussions.sourceDetail).mockResolvedValue(bound);
    vi.mocked(discussions.unlinkSourceSession).mockRejectedValue(new Error('offline'));
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Délier' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Échec déliaison');
  });

  it('survives a failed read without breaking the header', async () => {
    // The binding is optional metadata sitting in the chat header.
    vi.mocked(discussions.sourceDetail).mockRejectedValue(new Error('offline'));
    expect(() =>
      render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />),
    ).not.toThrow();
    await waitFor(() => expect(discussions.sourceDetail).toHaveBeenCalled());
    expect(screen.queryByRole('button')).toBeNull();
  });
});
