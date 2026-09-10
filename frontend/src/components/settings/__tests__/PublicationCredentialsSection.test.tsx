// KT-619 — what the operator sees, and what the screen never keeps.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { HumanCredential } from '../../../types/generated';

// `vi.mock` is hoisted above every declaration, so the mocks it closes over
// have to be created in a hoisted block too.
const { list, enrol, revoke, rotate } = vi.hoisted(() => ({
  list: vi.fn(),
  enrol: vi.fn(),
  revoke: vi.fn(),
  rotate: vi.fn(),
}));

vi.mock('../../../lib/api', () => ({
  publicationCredentials: { list, enrol, revoke, rotate },
}));

import { PublicationCredentialsSection } from '../PublicationCredentialsSection';

const ADMIN = 'kr-admin-operator';
const toast = vi.fn();
// Echo the key and its arguments: asserting on English prose only breaks the
// day somebody rewords a label.
const t = (key: string, ...args: (string | number)[]) => [key, ...args].join('|');

function credential(overrides: Partial<HumanCredential> = {}): HumanCredential {
  return {
    id: 'c-1',
    label: 'Romu — laptop',
    role: 'human',
    enrolled_by: 'admin',
    created_at: '2026-09-10T08:00:00Z',
    revoked_at: null,
    revoked_reason: null,
    ...overrides,
  } as HumanCredential;
}

function mount() {
  return render(<PublicationCredentialsSection toast={toast} t={t} />);
}

async function unlock(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText('settings.credentials.authorityLabel'), ADMIN);
  await user.click(screen.getByRole('button', { name: 'settings.credentials.unlock' }));
}

beforeEach(() => {
  [list, enrol, revoke, rotate, toast].forEach((mock) => mock.mockReset());
  list.mockResolvedValue([]);
});

afterEach(() => cleanup());

describe('PublicationCredentialsSection', () => {
  it('shows nothing until an authority is presented', async () => {
    mount();
    expect(screen.getByLabelText('settings.credentials.authorityLabel')).toBeInTheDocument();
    // The list is not fetched before it is unlocked: credential metadata is an
    // administration surface, so it is not loaded speculatively.
    expect(list).not.toHaveBeenCalled();
    expect(screen.queryByRole('table')).toBeNull();
  });

  it('says the same thing however the authority is wrong', async () => {
    const user = userEvent.setup();
    list.mockRejectedValue(new Error('forbidden'));
    mount();
    await unlock(user);

    await waitFor(() => expect(toast).toHaveBeenCalledWith('settings.credentials.refused', 'error'));
    // Telling the operator WHICH guard was tripped would tell an attacker what
    // to try next, so the screen stays on the same message and the same form.
    expect(screen.getByLabelText('settings.credentials.authorityLabel')).toBeInTheDocument();
  });

  it('never puts the authority anywhere it could outlive the page', async () => {
    const user = userEvent.setup();
    list.mockResolvedValue([credential()]);
    mount();
    await unlock(user);
    await screen.findByRole('table');

    // A secret that survives a reload is a secret sitting somewhere.
    expect(JSON.stringify(localStorage)).not.toContain(ADMIN);
    expect(JSON.stringify(sessionStorage)).not.toContain(ADMIN);
    expect(window.location.search).not.toContain(ADMIN);
    // And the field that took it is a password field, so it is not shoulder-read
    // or offered to a form filler.
    expect(screen.queryByDisplayValue(ADMIN)).toBeNull();
  });

  it('shows a minted secret once, and not in the list that follows', async () => {
    const user = userEvent.setup();
    list.mockResolvedValue([]);
    mount();
    await unlock(user);
    await screen.findByLabelText('settings.credentials.newLabel');

    const secret = 'kr-human-shown-once';
    enrol.mockResolvedValue({ credential: credential({ label: 'Phone' }), secret });
    list.mockResolvedValue([credential({ label: 'Phone' })]);

    await user.type(screen.getByLabelText('settings.credentials.newLabel'), 'Phone');
    await user.click(screen.getByRole('button', { name: 'settings.credentials.enrol' }));

    expect(await screen.findByText(secret)).toBeInTheDocument();
    // The row it created carries the label and never the secret.
    const table = screen.getByRole('table');
    expect(table.textContent).toContain('Phone');
    expect(table.textContent).not.toContain(secret);
  });

  it('enrols the role that was chosen, not a default', async () => {
    const user = userEvent.setup();
    mount();
    await unlock(user);
    await screen.findByLabelText('settings.credentials.newRole');

    enrol.mockResolvedValue({ credential: credential(), secret: 'kr-human-x' });
    await user.selectOptions(screen.getByLabelText('settings.credentials.newRole'), 'orchestrator');
    await user.type(screen.getByLabelText('settings.credentials.newLabel'), 'principal');
    await user.click(screen.getByRole('button', { name: 'settings.credentials.enrol' }));

    expect(enrol).toHaveBeenCalledWith(ADMIN, 'orchestrator', 'principal');
  });

  it('offers neither rotation nor revocation on a credential already revoked', async () => {
    const user = userEvent.setup();
    list.mockResolvedValue([
      credential({ label: 'Lost', revoked_at: '2026-09-10T09:00:00Z', revoked_reason: 'stolen' }),
    ]);
    mount();
    await unlock(user);
    await screen.findByRole('table');

    expect(screen.getByLabelText('settings.credentials.rotate|Lost')).toBeDisabled();
    expect(screen.getByLabelText('settings.credentials.revoke|Lost')).toBeDisabled();
    expect(screen.getByText('settings.credentials.stateRevoked|stolen')).toBeInTheDocument();
  });

  it('is operable from the keyboard alone', async () => {
    const user = userEvent.setup();
    mount();

    // The unlock form: field, then button, in reading order.
    await user.tab();
    expect(screen.getByLabelText('settings.credentials.authorityLabel')).toHaveFocus();
    await user.keyboard(ADMIN);
    await user.tab();
    expect(screen.getByRole('button', { name: 'settings.credentials.unlock' })).toHaveFocus();

    // Enter submits, exactly as clicking does.
    list.mockResolvedValue([credential()]);
    await user.keyboard('{Enter}');
    await screen.findByRole('table');
    expect(list).toHaveBeenCalledWith(ADMIN);
  });

  it('names every icon-only action, since the icon is not a name', async () => {
    const user = userEvent.setup();
    list.mockResolvedValue([credential({ label: 'Laptop' })]);
    mount();
    await unlock(user);
    await screen.findByRole('table');

    expect(screen.getByLabelText('settings.credentials.rotate|Laptop')).toBeInTheDocument();
    expect(screen.getByLabelText('settings.credentials.revoke|Laptop')).toBeInTheDocument();
  });

  it('mints one secret when two events land before a rerender', async () => {
    const user = userEvent.setup();
    mount();
    await unlock(user);
    await screen.findByLabelText('settings.credentials.newLabel');

    // Hold the call open so both clicks land while the first is still running.
    let release: (value: unknown) => void = () => {};
    enrol.mockReturnValue(new Promise((resolve) => { release = resolve; }));

    await user.type(screen.getByLabelText('settings.credentials.newLabel'), 'Phone');
    const button = screen.getByRole('button', { name: 'settings.credentials.enrol' });

    // Two synchronous submits. `busy` is React state and is not observable
    // until the next render, so a state-only gate lets both through — and each
    // one MINTS A SECRET.
    button.click();
    button.click();

    release({ credential: credential({ label: 'Phone' }), secret: 'kr-human-one' });
    await waitFor(() => expect(enrol).toHaveBeenCalledTimes(1));
  });

  it('rotates once when two clicks land before a rerender', async () => {
    const user = userEvent.setup();
    list.mockResolvedValue([credential({ label: 'Laptop' })]);
    mount();
    await unlock(user);
    await screen.findByRole('table');

    let release: (value: unknown) => void = () => {};
    rotate.mockReturnValue(new Promise((resolve) => { release = resolve; }));

    const button = screen.getByLabelText('settings.credentials.rotate|Laptop');
    button.click();
    button.click();

    release('kr-human-rotated');
    await waitFor(() => expect(rotate).toHaveBeenCalledTimes(1));
  });

  it('drops a displayed secret when the reload that follows is refused', async () => {
    const user = userEvent.setup();
    mount();
    await unlock(user);
    await screen.findByLabelText('settings.credentials.newLabel');

    const secret = 'kr-human-possibly-stale';
    enrol.mockResolvedValue({ credential: credential({ label: 'Phone' }), secret });
    // The reload right after fails: we no longer know whether what is on screen
    // is current, and a stale secret shown as usable is worse than none.
    list.mockRejectedValue(new Error('forbidden'));

    await user.type(screen.getByLabelText('settings.credentials.newLabel'), 'Phone');
    await user.click(screen.getByRole('button', { name: 'settings.credentials.enrol' }));

    await waitFor(() => expect(screen.queryByText(secret)).toBeNull());
  });

  it('tells the operator that nothing publishes until something is enrolled', async () => {
    const user = userEvent.setup();
    list.mockResolvedValue([]);
    mount();
    await unlock(user);
    // The fail-closed state is the one worth explaining: silence here would
    // read as a bug rather than as a deliberate refusal.
    expect(await screen.findByText('settings.credentials.empty')).toBeInTheDocument();
  });
});
