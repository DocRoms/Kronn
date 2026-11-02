import { createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test, expect } from '../fixtures/kronn-fixture';

// KT-1492 — Settings › Trust levels drives the real trust API: a level above
// Standard goes through the risk panel, an explicit confirmation and a code from
// an authenticator enrolled in the same screen; an active level shows a badge,
// and one click revokes it. Run against a backend with an isolated
// KRONN_DATA_DIR: this spec enrols an authenticator, changes the instance's trust
// level, and puts both back. A host whose OS cannot sandbox what a level
// approves (no bubblewrap) still allows a level: the panel then names the missing
// vault protection and asks for its own acknowledgement, which the spec ticks.
// The first enrolment is authorised by the admin secret (the human-admin-secret
// file of the data directory): the spec reads it from KRONN_SANDBOX_DIR or
// KRONN_DATA_DIR and skips when neither is set.

const BASE32 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';

/** RFC 6238, SHA-1, 6 digits, 30 s: what any authenticator app shows. */
function totp(secret: string, stepOffset = 0): string {
  let bits = '';
  for (const char of secret.replace(/\s+/g, '').toUpperCase()) bits += BASE32.indexOf(char).toString(2).padStart(5, '0');
  const key = Buffer.from((bits.match(/.{8}/g) ?? []).map(byte => parseInt(byte, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30_000) + stepOffset));
  const mac = createHmac('sha1', key).update(counter).digest();
  const offset = mac[19] & 0x0f;
  const value = ((mac[offset] & 0x7f) << 24) | (mac[offset + 1] << 16) | (mac[offset + 2] << 8) | mac[offset + 3];
  return String(value % 1_000_000).padStart(6, '0');
}

/** The bootstrap secret, read where the backend delivered it. */
function adminSecret(): string | null {
  const dir = process.env.KRONN_SANDBOX_DIR ?? process.env.KRONN_DATA_DIR;
  if (!dir) return null;
  try {
    return readFileSync(join(dir, 'human-admin-secret'), 'utf8').trim();
  } catch {
    return null;
  }
}

test.describe.configure({ mode: 'serial' });

test.describe('trust levels (KT-1492)', () => {
  let recovery: string[] = [];
  let used = 0;
  /** Every proof is a recovery code: a code is accepted once, and the authenticator's own
   *  next code is 30 s away. */
  const nextProof = () => recovery[used++];
  /** The Kronn vault is not protected on this host (no sandbox, or the setting is off). */
  let unprotected = false;

  test.beforeAll(async ({ request }) => {
    const overview = await request.get('/api/trust');
    const data = (await overview.json()).data;
    unprotected = !data.sandbox.protected;
    test.skip(data.proof.enrolled, 'an authenticator is already enrolled on this instance');
    test.skip(adminSecret() === null, 'the admin secret is not readable: set KRONN_SANDBOX_DIR or KRONN_DATA_DIR');
  });

  test.afterAll(async ({ request }) => {
    await request.put('/api/trust/instance', { data: { level: 'standard', acknowledged_risks: false } });
    if (recovery[used]) {
      await request.post('/api/trust/proof/disable', { data: { proof: recovery[used] } });
    }
  });

  test('enrols an authenticator: QR, manual key, first code, recovery codes shown once', async ({ page }) => {
    await page.goto('/config#settings-trust');
    const section = page.getByTestId('trust-proof-section');
    await expect(section.getByTestId('trust-proof-status')).toBeVisible();

    await section.getByTestId('trust-proof-admin-secret').fill(adminSecret() ?? '');
    await section.getByTestId('trust-proof-begin').click();
    await expect(section.getByTestId('trust-proof-qr')).toBeVisible();
    const secret = (await section.getByTestId('trust-proof-secret').innerText()).trim();
    expect(secret.replace(/\s+/g, '')).toMatch(/^[A-Z2-7]{32}$/);

    await section.getByTestId('trust-proof-code').fill(totp(secret));
    await section.getByTestId('trust-proof-confirm').click();

    const codes = section.getByTestId('trust-proof-recovery').locator('li');
    await expect(codes).toHaveCount(10);
    recovery = await codes.allInnerTexts();
    await section.getByTestId('trust-proof-saved').click();
    await expect(section.getByTestId('trust-proof-recovery')).toBeHidden();
  });

  test('Trusted needs the risk panel, its confirmation and a code, shows a badge and revokes in one click', async ({ page }) => {
    await page.goto('/config#settings-trust');
    const section = page.getByTestId('settings-trust');
    await expect(section).toBeVisible();
    await expect(page.getByTestId('trust-badge-instance')).toBeHidden();

    await section.getByTestId('trust-level-trusted').click();
    const panel = page.getByTestId('trust-risk-panel');
    await expect(panel).toBeVisible();
    // The concrete effects, not a generic warning.
    await expect(panel.locator('[data-approval="quick_exec_line"]')).toBeVisible();
    await expect(panel.locator('[data-approval="validations_docker"]')).toBeVisible();
    await expect(panel.locator('[data-approval="validations_unconfined"]')).toHaveCount(0);
    await expect(panel.getByTestId('trust-never')).toBeVisible();
    // What stays reachable and what the sandbox closes.
    // The network and Kronn's own API stay open; only Kronn's data directory closes.
    await expect(panel.getByTestId('trust-reachable').locator('li')).toHaveCount(6);
    await expect(panel.getByTestId('trust-closed').locator('li')).toHaveCount(1);

    // Nothing is saved until the explicit confirmation and the code.
    const confirm = panel.getByTestId('trust-confirm');
    await expect(confirm).toBeDisabled();
    await panel.getByTestId('trust-ack').check();
    await expect(confirm).toBeDisabled();
    const before = await page.request.get('/api/trust/instance');
    expect((await before.json()).data.effective.level).toBe('standard');

    await panel.getByTestId('trust-proof-input').fill(nextProof());
    // Without a sandbox the missing vault protection has its own acknowledgement.
    if (unprotected) {
      await expect(panel.getByTestId('trust-vault-unprotected')).toBeVisible();
      await expect(confirm).toBeDisabled();
      await panel.getByTestId('trust-vault-ack').check();
    }
    await confirm.click();
    await expect(panel).toBeHidden();

    const badge = page.getByTestId('trust-badge-instance');
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute('data-level', 'trusted');
    const after = await page.request.get('/api/trust/instance');
    expect((await after.json()).data.effective.level).toBe('trusted');

    await page.getByTestId('trust-revoke').click();
    await expect(badge).toBeHidden();
    const revoked = await page.request.get('/api/trust/instance');
    expect((await revoked.json()).data.effective.level).toBe('standard');
  });

  test('a wrong code keeps the panel open and the level unchanged', async ({ page }) => {
    await page.goto('/config#settings-trust');
    await page.getByTestId('trust-level-full').click();
    const panel = page.getByTestId('trust-risk-panel');
    await panel.getByTestId('trust-ack').check();
    await panel.getByTestId('trust-proof-input').fill('000000');
    if (unprotected) await panel.getByTestId('trust-vault-ack').check();
    await panel.getByTestId('trust-confirm').click();
    await expect(panel).toBeVisible();
    const level = await page.request.get('/api/trust/instance');
    expect((await level.json()).data.effective.level).toBe('standard');
    await panel.getByTestId('trust-cancel').click();
  });

  test('Full access lists what Trusted does and more', async ({ page }) => {
    await page.goto('/config#settings-trust');
    await page.getByTestId('trust-level-full').click();
    const panel = page.getByTestId('trust-risk-panel');
    for (const approval of ['quick_exec_line', 'validations_docker', 'validations_unconfined', 'code_write_other_project']) {
      await expect(panel.locator(`[data-approval="${approval}"]`)).toBeVisible();
    }
    await panel.getByTestId('trust-cancel').click();
    await expect(panel).toBeHidden();
    const level = await page.request.get('/api/trust/instance');
    expect((await level.json()).data.effective.level).toBe('standard');
  });

  test('the server refuses a level above Standard without the confirmation or without a proof', async ({ request }) => {
    const noPanel = await request.put('/api/trust/instance', {
      data: { level: 'full', acknowledged_risks: false, proof: nextProof() },
    });
    expect(noPanel.status()).toBe(400);
    // A request from this very machine, with the panel's confirmation but no code,
    // is what a command an agent approved could send: it changes nothing.
    for (const proof of [undefined, '000000']) {
      const noProof = await request.put('/api/trust/instance', {
        data: { level: 'full', acknowledged_risks: true, proof },
      });
      expect(noProof.status()).toBe(403);
    }
    const level = await request.get('/api/trust/instance');
    expect((await level.json()).data.effective.level).toBe('standard');
  });

  test('the code is the proof whatever the channel: the X-Kronn-Confirm header works once', async ({ request }) => {
    const code = nextProof();
    const raised = await request.put('/api/trust/instance', {
      headers: { 'X-Kronn-Confirm': code },
      data: { level: 'trusted', acknowledged_risks: true, acknowledged_vault_unprotected: true },
    });
    expect(raised.status()).toBe(200);
    // Spent: the same code is a replay.
    const replay = await request.put('/api/trust/instance', {
      headers: { 'X-Kronn-Confirm': code },
      data: { level: 'full', acknowledged_risks: true, acknowledged_vault_unprotected: true },
    });
    expect(replay.status()).toBe(403);
    // The journal names the channel.
    const audit = (await (await request.get('/api/trust/audit')).json()).data as { event: string; detail: string }[];
    expect(audit.some(line => line.event === 'proof' && line.detail.includes('http channel'))).toBe(true);
    await request.put('/api/trust/instance', { data: { level: 'standard', acknowledged_risks: false } });
  });
});
