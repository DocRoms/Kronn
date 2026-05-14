// Coverage for the static plugin-tips registry that the AI helper injects
// into its system prompt. We don't snapshot the full lore (it's prose that
// will iterate) — we lock the invariants:
//   1. Lookup-by-slug works and is null-safe.
//   2. Tips registered for `mcp-resend` / `api-mailjet` carry the
//      operational landmines an agent NEEDS to avoid burning a workflow
//      run on a 422 (verified domain, validated sender, etc.).
// If the tips body drifts and loses one of these landmines, the workflow
// AI helper will start generating broken ApiCall steps for first-time
// users — exactly the regression this test guards.

import { describe, it, expect } from 'vitest';
import { PLUGIN_TIPS, tipsForSlug } from '../apiCallPluginTips';

describe('tipsForSlug', () => {
  it('returns null for missing slugs (null, undefined, unknown)', () => {
    expect(tipsForSlug(null)).toBeNull();
    expect(tipsForSlug(undefined)).toBeNull();
    expect(tipsForSlug('')).toBeNull();
    expect(tipsForSlug('does-not-exist')).toBeNull();
  });

  it('returns the registered tips for known slugs', () => {
    const chartbeat = tipsForSlug('chartbeat');
    expect(chartbeat).not.toBeNull();
    expect(chartbeat?.body.length).toBeGreaterThan(0);
    expect(chartbeat?.docsUrl).toBeDefined();
  });
});

// Resend is registered under `mcp-resend` even though the tips lore is
// API-flavoured — same convention as mcp-github / mcp-atlassian, the
// hybrid plugin keeps a single id and `tipsForSlug(server.id)` looks
// up by that id. The test name keeps the `mcp-resend` slug to avoid
// drift between the test description and the actual lookup key.
describe('PLUGIN_TIPS — mcp-resend (hybrid MCP + API tips)', () => {
  const resend = PLUGIN_TIPS['mcp-resend'];

  it('is registered with a non-empty body + docs URL', () => {
    expect(resend).toBeDefined();
    expect(resend.body.length).toBeGreaterThan(200);
    expect(resend.docsUrl).toContain('resend.com');
  });

  it('warns about verified domain (the #1 422 trap)', () => {
    expect(resend.body).toMatch(/domaine vérifié|verified/i);
    expect(resend.body).toMatch(/from address is not valid|domaines/i);
  });

  it('mentions idempotency for CSM replay-safety', () => {
    expect(resend.body).toMatch(/Idempotency-Key/);
  });

  it('clarifies the array-vs-string traps (to is array, reply_to is string)', () => {
    expect(resend.body).toMatch(/reply_to/);
    expect(resend.body).toMatch(/array|tableau/i);
  });

  it('documents both /emails and /emails/batch with their constraints', () => {
    expect(resend.body).toMatch(/\/emails\/batch/);
    expect(resend.body).toMatch(/100/);
  });
});

describe('PLUGIN_TIPS — api-mailjet', () => {
  const mailjet = PLUGIN_TIPS['api-mailjet'];

  it('is registered with a non-empty body + docs URL', () => {
    expect(mailjet).toBeDefined();
    expect(mailjet.body.length).toBeGreaterThan(200);
    expect(mailjet.docsUrl).toContain('mailjet');
  });

  it('flags the "Sender not allowed" pitfall (Mailjet #1 400 cause)', () => {
    expect(mailjet.body).toMatch(/Sender not allowed|sender validé|REST\/sender/);
  });

  it('directs to v3.1/send (modern) and warns against v3 legacy', () => {
    expect(mailjet.body).toMatch(/\/v3\.1\/send/);
    expect(mailjet.body).toMatch(/v3\/send|legacy/i);
  });

  it('warns that a 200 OK can hide per-message failures', () => {
    expect(mailjet.body).toMatch(/Messages\[\]\.Status|Status/);
    expect(mailjet.body).toMatch(/200/);
  });

  it('mentions SandboxMode (dry-run for Gate previews)', () => {
    expect(mailjet.body).toMatch(/SandboxMode/);
  });

  it('exposes managecontact as the CSM segmentation primitive', () => {
    expect(mailjet.body).toMatch(/managecontact/);
  });
});
