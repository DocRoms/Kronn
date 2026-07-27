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
  'disc.session.idPlaceholder': 'ID fourni',
  'disc.session.required': 'Champs obligatoires',
  'disc.session.alreadyLinked': 'Déjà liée à #{0}',
  'disc.session.linked': 'Session liée',
  'disc.session.unlinked': 'Session déliée',
  'disc.session.linkFailed': 'Échec liaison',
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

  it('links an unowned session and emits the sidebar refresh event', async () => {
    const changed = vi.fn();
    window.addEventListener('kronn:disc-source-changed', changed);
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    await waitFor(() => expect(discussions.sourceDetail).toHaveBeenCalledWith('disc-a'));

    fireEvent.click(screen.getByRole('button', { name: 'Session' }));
    fireEvent.change(screen.getByLabelText('ID de session'), {
      target: { value: 'codex-session-42' },
    });
    fireEvent.change(screen.getByLabelText('CLI source'), {
      target: { value: 'Codex' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Lier une session' }));

    await waitFor(() => expect(discussions.linkSourceSession).toHaveBeenCalledWith({
      disc_id: 'disc-a',
      source_agent: 'Codex',
      source_session_id: 'codex-session-42',
      force_reassign: false,
    }));
    expect(changed).toHaveBeenCalledOnce();
    expect(toast).toHaveBeenCalledWith('Session liée', 'success');
    window.removeEventListener('kronn:disc-source-changed', changed);
  });

  it('refuses to steal a session already linked elsewhere', async () => {
    vi.mocked(discussions.sourceSessionStatus).mockResolvedValue({
      binding_version: 1,
      bound_disc_id: 'disc-other-1234',
      connected_disc_id: null,
      connection_status: null,
    });
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Session' }));
    fireEvent.change(screen.getByLabelText('ID de session'), {
      target: { value: 'shared-session' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Lier une session' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Déjà liée à #disc-oth');
    expect(discussions.linkSourceSession).not.toHaveBeenCalled();
  });

  it('shows offline state, copies the full id and can unlink', async () => {
    vi.mocked(discussions.sourceDetail).mockResolvedValue({
      current: {
        binding_version: 1,
        disc_id: 'disc-a',
        source_agent: 'ClaudeCode',
        source_session_id: 'claude-session-full-id',
        imported_at: null,
        diverged_at: null,
      },
      history: [],
    });
    render(<DiscussionSessionBinding discussionId="disc-a" toast={toast} t={t} />);
    fireEvent.click(await screen.findByRole('button', { name: /ClaudeCode · claude-s/ }));

    expect(await screen.findByText('Hors ligne ou non détectée')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copier claude-session-full-id' })).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Délier' }));

    await waitFor(() => expect(discussions.unlinkSourceSession).toHaveBeenCalledWith('disc-a'));
    expect(toast).toHaveBeenCalledWith('Session déliée', 'success');
  });
});
