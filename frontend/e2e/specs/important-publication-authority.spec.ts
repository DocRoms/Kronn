/**
 * KT-619 — rights, publication and navigation, against a REAL isolated stack.
 *
 * The layout fixture next door (`important-message-card-layout.spec.ts`) draws
 * markup into a blank page. This one drives the whole thing: a backend on its
 * own port over a temporary `KRONN_DATA_DIR`, its own Vite, a browser, and the
 * real database underneath.
 *
 * # Why it refuses to run by default
 *
 * It ENROLS CREDENTIALS and ROTATES THE PUBLICATION BOOTSTRAP. Run against the
 * instance somebody is working in, that is not a test — it is damage, and the
 * damage is silent: the operator's admin secret stops working and the file they
 * saved is stale. So the spec runs only when handed a sandbox directory, and
 * refuses outright on the port a developer actually uses.
 *
 * Setup is in `e2e/README.md` under "Isolated publication-authority run".
 */

import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { assertPublicationSandbox } from '../fixtures/publication-sandbox.mjs';

const SANDBOX = process.env.KRONN_SANDBOX_DIR ?? '';

/** The bootstrap, read where the backend delivered it rather than injected. */
function adminSecret(): string {
  return readFileSync(join(SANDBOX, 'human-admin-secret'), 'utf8').trim();
}

/** A message carrying one card. The JSON is the contract, not a convention. */
function fence(dedupKey: string, highlight: string): string {
  return [
    'Point de passage.',
    '',
    '```kronn-important',
    JSON.stringify(
      {
        version: 1,
        category: 'decision',
        dedup_key: dedupKey,
        title: 'Le socle de publication part en 0.13.0',
        highlight,
        impact: 'Une install sans identifiant enrôlé ne publie aucune carte.',
        action_required: { required: false },
      },
      null,
      2,
    ),
    '```',
  ].join('\n');
}

let discId = '';
let grant = '';

test.describe.serial('publication authority, end to end', () => {
  // No retry. The default config retries once, and a retry here re-runs the
  // whole serial block against a database the first attempt already changed:
  // the credential is enrolled, the bootstrap is rotated, and the second run
  // fails on assertions that were true only the first time. A flake in this
  // spec is a result, not noise to be re-rolled.
  test.describe.configure({ retries: 0 });

  test.skip(!SANDBOX, 'needs an isolated backend — see e2e/README.md (KRONN_SANDBOX_DIR)');

  test.beforeAll(() => {
    assertPublicationSandbox();
  });

  test.afterAll(async ({ request }) => {
    if (discId) await request.delete(`/api/discussions/${discId}`).catch(() => {});
  });

  test('an install with no credential posts the message and publishes no card', async ({
    page,
    request,
  }) => {
    const created = await request.post('/api/discussions', {
      data: {
        title: `KT-619 autorité PW ${Date.now()}`,
        agent: 'ClaudeCode',
        language: 'fr',
        initial_prompt: 'Ne lance aucun modèle.',
        no_agent: true,
      },
    });
    expect(created.ok()).toBe(true);
    discId = (await created.json())?.data?.id ?? '';
    expect(discId).toBeTruthy();

    // No grant exists on a fresh install. The fence is well-formed and the
    // publisher is simply nobody — which must lose the CARD and keep the
    // MESSAGE, rather than refuse the whole send.
    const posted = await request.post(`/api/discussions/${discId}/messages`, {
      data: { content: fence('e2e-fail-closed', 'Rien ne se publie avant enrôlement.') },
    });
    expect(posted.ok()).toBe(true);

    const listed = await request.get(`/api/discussions/${discId}/important`);
    expect(listed.ok()).toBe(true);
    expect((await listed.json())?.data?.total_all).toBe(0);

    // And the reader is told, rather than shown raw JSON or nothing at all.
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(discId);
    await expect(page.getByText('Cette carte n’a pas été enregistrée')).toBeVisible();
    // No card anywhere means no bar either: a counter reading zero would be
    // furniture for a feature this install does not have.
    await expect(page.locator('.disc-important-bar')).toHaveCount(0);
  });

  test('the admin secret opens the credential surface and a wrong one does not', async ({
    page,
  }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.getByRole('region', { name: 'Identifiants de publication' });
    await expect(section).toBeVisible();
    // The person who cannot get past this form is the one no button here can
    // serve, so the way back is named on the screen they are stuck on.
    await expect(section.getByText(/recover-admin-secret/)).toBeVisible();

    const field = section.getByLabel('Secret admin ou identifiant humain');
    await field.fill('kr-admin-wrong-on-purpose');
    await section.getByRole('button', { name: 'Déverrouiller' }).click();
    await expect(page.getByText('Cette autorité a été refusée.')).toBeVisible();
    // Refused means still locked: no table, no enrolment form.
    await expect(section.getByRole('table')).toHaveCount(0);

    await field.fill(adminSecret());
    await section.getByRole('button', { name: 'Déverrouiller' }).click();
    await expect(section.getByRole('table')).toBeVisible();
    await expect(
      section.getByText('Aucun identifiant enrôlé.', { exact: false }),
    ).toBeVisible();
  });

  test('an enrolled credential publishes a card the transcript can reach', async ({
    page,
    request,
  }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.getByRole('region', { name: 'Identifiants de publication' });
    await section.getByLabel('Secret admin ou identifiant humain').fill(adminSecret());
    await section.getByRole('button', { name: 'Déverrouiller' }).click();

    await section.getByLabel('Nom', { exact: true }).fill('Poste E2E');
    await section.getByRole('button', { name: 'Enrôler' }).click();

    // Shown once, here, and nowhere afterwards.
    const shown = section.locator('code').first();
    await expect(shown).toBeVisible();
    grant = ((await shown.textContent()) ?? '').trim();
    expect(grant).toMatch(/^kr-human-/);
    expect((await section.getByRole('table').textContent()) ?? '').not.toContain(grant);

    // Publish from the COMPOSER, which is the whole point: enrolling a grant
    // in Settings and then reaching for curl is not a path a person has.
    await dashboard.goto();
    await dashboard.openDiscussion(discId);

    await page.getByText('Publier une carte importante en votre nom').click();
    await page.getByLabel('Identifiant de publication').fill(grant);

    const content = fence('e2e-published', 'La carte part avec un identifiant enrôlé.');
    await page.locator('.disc-composer-textarea').fill(content);
    await page.locator('.disc-send-btn').first().click();

    await expect(async () => {
      const listed = await request.get(`/api/discussions/${discId}/important`);
      const list = (await listed.json())?.data;
      expect(list?.total_all).toBe(1);
      // Signed as a human, not as the orchestrator: the roles exist to be
      // distinguishable and a card saying "human" must mean one.
      expect(list?.items?.[0]?.author_kind).toBe('human');
    }).toPass({ timeout: 15_000 });

    // Navigation: the bar counts what the database holds, and its counter
    // reaches the card even with both arrows disabled at one item.
    const bar = page.locator('.disc-important-bar');
    await expect(bar).toBeVisible();
    await expect(bar.getByRole('button', { name: 'Aller au message important courant' })).toHaveText(
      /1/,
    );
    await expect(bar.getByRole('button', { name: 'Message important suivant' })).toBeDisabled();

    await bar.getByRole('button', { name: 'Aller au message important courant' }).click();
    const card = page.locator('.disc-important-card[data-category="decision"]');
    await expect(card).toBeVisible();
    await expect(card).toContainText('La carte part avec un identifiant enrôlé.');

    // Filtering to a category that holds nothing must not claim the card is
    // gone: the counter reads the discussion, the position reads the filter.
    await bar.getByLabel('Filtrer par catégorie').selectOption('blocking_alert');
    await expect(bar.getByRole('button', { name: 'Aller au message important courant' })).toHaveText(
      /1/,
    );
    await expect(bar.locator('.disc-important-position')).toHaveText('Aucun dans cette catégorie');
  });

  test('a restored outbox obtains fresh authority and preserves its message identity', async ({ page, request }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    const content = fence('e2e-queued', 'La reprise de la file conserve é🙂 et son identité.');
    const clientMessageId = randomUUID();
    // Persist the same shape a tab leaves after an interrupted send. No grant
    // or proof is seeded, and the real hook/HTTP writer perform the retry.
    await page.evaluate(({ room, content, id }) => {
      localStorage.setItem(`kronn:message-outbox:${room}`, JSON.stringify([{
        id, content, status: 'failed', attempts: 1, createdAt: new Date().toISOString(),
      }]));
    }, { room: discId, content, id: clientMessageId });
    await dashboard.openDiscussion(discId);
    await expect(page.locator('.disc-queued-line')).toHaveCount(1);
    await page.getByText('Publier une carte importante en votre nom').click();
    await page.getByLabel('Identifiant de publication').fill(grant);
    const proofRequest = page.waitForRequest(req => req.url().endsWith('/api/human-credentials/proof')
      && req.method() === 'POST' && req.postDataJSON()?.content === content);
    const sendRequest = page.waitForRequest(req => req.url().endsWith(`/api/discussions/${discId}/messages`)
      && req.method() === 'POST' && req.postDataJSON()?.client_message_id === clientMessageId);
    await page.locator('.disc-queued-line').getByRole('button', { name: 'Réessayer' }).click();
    const proofBody = (await proofRequest).postDataJSON();
    expect(proofBody.discussion_id).toBe(discId);
    const sent = (await sendRequest).postDataJSON();
    expect(sent.defer_dispatch).toBe(true);
    expect(sent.content).toBe(content);
    expect(sent.publication_proof).toBeTruthy();
    await expect(page.locator('.disc-queued-line')).toHaveCount(0);
    await expect(async () => {
      const response = await request.get(`/api/discussions/${discId}/important`);
      expect((await response.json())?.data?.total_all).toBe(2);
    }).toPass({ timeout: 15_000 });
    expect(await page.evaluate(room => localStorage.getItem(`kronn:message-outbox:${room}`), discId)).toBeNull();
    await page.reload();
    await dashboard.openDiscussion(discId);
    await expect(page.getByLabel('Identifiant de publication')).toHaveValue('');
    await expect(page.locator('.disc-queued-line')).toHaveCount(0);
    const listed = await request.get(`/api/discussions/${discId}/important`);
    expect((await listed.json())?.data?.total_all).toBe(2);
  });

  test('a proof is spent once, whatever presents it again', async ({ request }) => {
    // The composer mints a fresh proof per send, so replay is not something a
    // person can do through the UI — which is exactly why it is checked here.
    const content = fence('e2e-replay', 'Une preuve ne se dépense qu’une fois.');
    const proof = await request.post('/api/human-credentials/proof', {
      data: { grant, discussion_id: discId, content },
    });
    expect(proof.ok()).toBe(true);
    const proofId = (await proof.json())?.data;

    for (const attempt of [1, 2]) {
      const posted = await request.post(`/api/discussions/${discId}/messages`, {
        data: { content, publication_grant: grant, publication_proof: proofId },
      });
      expect(posted.ok(), `send ${attempt} must still post the message`).toBe(true);
    }

    // Two sends, one card: the second proof was already spent. The MESSAGE
    // posted both times — a refused publication never costs the text.
    const listed = await request.get(`/api/discussions/${discId}/important`);
    expect((await listed.json())?.data?.total_all).toBe(3);
  });

  test('switching rooms restores the selected card without borrowing another room position', async ({ page, request }) => {
    const created = await request.post('/api/discussions', {
      data: { title: 'KT-619 autre salle navigation', agent: 'ClaudeCode', language: 'fr', initial_prompt: 'Ne lance aucun modèle.', no_agent: true },
    });
    expect(created.ok()).toBe(true);
    const other = (await created.json())?.data?.id;
    expect(other).toBeTruthy();
    try {
      const content = fence('e2e-other-room', 'La sélection appartient à cette salle.');
      const proof = await request.post('/api/human-credentials/proof', {
        data: { grant, discussion_id: other, content },
      });
      expect(proof.ok()).toBe(true);
      const posted = await request.post(`/api/discussions/${other}/messages`, {
        data: { content, publication_grant: grant, publication_proof: (await proof.json())?.data },
      });
      expect(posted.ok()).toBe(true);
      const dashboard = new DashboardPage(page);
      await dashboard.goto();
      await dashboard.openDiscussion(discId);
      const bar = page.locator('.disc-important-bar');
      await expect(bar.locator('.disc-important-position')).toHaveText('1 sur 3');
      await bar.getByRole('button', { name: 'Message important suivant' }).click();
      await bar.getByRole('button', { name: 'Message important suivant' }).click();
      await expect(bar.locator('.disc-important-position')).toHaveText('3 sur 3');
      await dashboard.openDiscussion(other);
      await expect(bar.locator('.disc-important-position')).toHaveText('1 sur 1');
      await expect(bar.getByRole('button', { name: 'Message important précédent' })).toBeDisabled();
      await dashboard.openDiscussion(discId);
      await expect(bar.locator('.disc-important-position')).toHaveText('3 sur 3');
      await bar.getByRole('button', { name: 'Aller au message important courant' }).click();
      await expect(page.locator('article.disc-important-card').filter({ hasText: 'Une preuve ne se dépense qu’une fois.' })).toBeVisible();
    } finally {
      await request.delete(`/api/discussions/${other}`);
    }
  });

  test('rotating the bootstrap locks the screen back and retires the old secret', async ({
    page,
  }) => {
    const before = adminSecret();
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.getByRole('region', { name: 'Identifiants de publication' });
    await section.getByLabel('Secret admin ou identifiant humain').fill(before);
    await section.getByRole('button', { name: 'Déverrouiller' }).click();
    await expect(section.getByRole('table')).toBeVisible();

    page.once('dialog', (dialog) => dialog.accept());
    await section.getByRole('button', { name: 'Renouveler le secret admin' }).click();

    // A path, never a secret — and the screen goes back to the lock, because
    // the authority just typed no longer authenticates anything.
    await expect(section.getByText('Un nouveau secret admin a été écrit dans :')).toBeVisible();
    await expect(section.getByRole('table')).toHaveCount(0);

    // The file on disk moved, and the old value is finished.
    await expect(async () => expect(adminSecret()).not.toBe(before)).toPass({ timeout: 5_000 });
    const after = adminSecret();
    expect(section.locator('code').first()).not.toContainText(after);

    await section.getByLabel('Secret admin ou identifiant humain').fill(before);
    await section.getByRole('button', { name: 'Déverrouiller' }).click();
    await expect(page.getByText('Cette autorité a été refusée.')).toBeVisible();

    await section.getByLabel('Secret admin ou identifiant humain').fill(after);
    await section.getByRole('button', { name: 'Déverrouiller' }).click();
    // Rotation moves the power to enrol NEW credentials. It is not a
    // revocation sweep: the one enrolled two tests ago is still listed.
    await expect(section.getByRole('table')).toContainText('Poste E2E');
  });
});
