import { describe, it, expect } from 'vitest';
import {
  redactSecrets,
  sanitizeLogLines,
  buildIssueTitle,
  buildIssueBody,
  buildIssueUrl,
  KRONN_REPO_URL,
} from '../bug-report';

describe('redactSecrets', () => {
  it('redacts Anthropic/OpenAI sk-* style keys', () => {
    const out = redactSecrets('loaded key sk-ant-abcDEF123456789012345 from env');
    expect(out).toContain('sk-***REDACTED***');
    expect(out).not.toContain('sk-ant-abcDEF123456789012345');
  });

  it('redacts GitHub tokens in all common prefixes', () => {
    // Using generic placeholders that match the pattern shape without
    // being a real token — keeps the feedback_no_real_names rule happy.
    const cases = [
      'ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
      'gho_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBB',
      'ghs_CCCCCCCCCCCCCCCCCCCCCCCCCCCCCC',
      'ghu_DDDDDDDDDDDDDDDDDDDDDDDDDDDDDD',
    ];
    for (const raw of cases) {
      const out = redactSecrets(`token=${raw}`);
      expect(out).toBe('token=gh*_***REDACTED***');
    }
  });

  it('redacts Google API keys starting with AIza', () => {
    const out = redactSecrets('GEMINI_API_KEY=AIza1234567890abcdefghijklmnopqrstuv');
    expect(out).toContain('AIza***REDACTED***');
  });

  it('redacts Bearer tokens in headers', () => {
    const out = redactSecrets('Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9');
    expect(out).toBe('Authorization: Bearer ***REDACTED***');
  });

  it('redacts JSON token/password/api_key fields', () => {
    expect(redactSecrets('{"password":"hunter2plus"}'))
      .toBe('{"password":"***REDACTED***"}');
    expect(redactSecrets('{"api_key":"xyz-987-abc"}'))
      .toBe('{"api_key":"***REDACTED***"}');
    expect(redactSecrets('{"apiKey":"some-long-string"}'))
      .toBe('{"apiKey":"***REDACTED***"}');
  });

  it('leaves non-sensitive text alone', () => {
    const plain = '22:15:03.121  INFO kronn::scanner — scanning /Users/a/Code';
    expect(redactSecrets(plain)).toBe(plain);
  });
});

describe('sanitizeLogLines', () => {
  it('applies redaction across every line', () => {
    const input = [
      'line 1 with sk-ant-1234567890ABCDEFGHIJ token',
      'line 2 clean',
      'Authorization: Bearer 1234567890ABCDEF123456',
    ];
    const out = sanitizeLogLines(input);
    expect(out[0]).toContain('sk-***REDACTED***');
    expect(out[1]).toBe('line 2 clean');
    expect(out[2]).toContain('Bearer ***REDACTED***');
  });

  it('preserves order and length', () => {
    const input = ['a', 'b', 'c'];
    expect(sanitizeLogLines(input)).toEqual(input);
  });
});

describe('buildIssueTitle', () => {
  it('stamps version + host_os when available', () => {
    expect(buildIssueTitle({ kronnVersion: '0.4.0', hostOs: 'macOS', logLines: [] }))
      .toBe('[Bug] Kronn v0.4.0 on macOS');
  });

  it('degrades gracefully when env info is missing', () => {
    expect(buildIssueTitle({ logLines: [] })).toBe('[Bug] Kronn');
  });
});

describe('buildIssueBody', () => {
  it('includes env info, agent summary, and log tail', () => {
    const body = buildIssueBody({
      kronnVersion: '0.4.0',
      hostOs: 'WSL',
      agentsSummary: ['Claude Code: ok v2.3.0', 'Gemini CLI: missing'],
      logLines: ['22:15:03 INFO kronn — boot', '22:15:04 INFO kronn::scanner — ok'],
      userContext: 'I clicked the audit button and nothing happened',
      userAgent: 'Mozilla/5.0 (X11; Linux x86_64) Firefox/125',
    });
    expect(body).toContain('Kronn version: 0.4.0');
    expect(body).toContain('Host OS: WSL');
    expect(body).toContain('Mozilla/5.0');
    expect(body).toContain('clicked the audit button');
    expect(body).toContain('Claude Code: ok v2.3.0');
    expect(body).toContain('Gemini CLI: missing');
    expect(body).toContain('22:15:03 INFO kronn — boot');
  });

  it('keeps only the last N log lines when capped', () => {
    const lines = Array.from({ length: 500 }, (_, i) => `line-${i}`);
    const body = buildIssueBody({ logLines: lines }, 10);
    // Last 10 kept, earlier dropped.
    expect(body).toContain('line-499');
    expect(body).toContain('line-490');
    expect(body).not.toContain('line-100');
    expect(body).toContain('last 10 lines');
  });

  it('substitutes a neutral placeholder when user context is empty', () => {
    const body = buildIssueBody({ logLines: [] });
    expect(body).toContain('<!-- Please describe');
  });

  it('reports "(no logs captured)" when the tail is empty', () => {
    const body = buildIssueBody({ logLines: [] });
    expect(body).toContain('(no logs captured)');
  });

  it('redacts secrets in the embedded log section', () => {
    const body = buildIssueBody({
      logLines: ['loaded sk-ant-ABCDEFGHIJKLMNOPQRSTUVWX123 from env'],
    });
    expect(body).not.toContain('sk-ant-ABCDEFGHIJKLMNOPQRSTUVWX123');
    expect(body).toContain('sk-***REDACTED***');
  });
});

describe('buildIssueUrl', () => {
  it('points at the canonical Kronn repo with the bug label', () => {
    const url = buildIssueUrl({ logLines: [] });
    expect(url.startsWith(`${KRONN_REPO_URL}/issues/new?`)).toBe(true);
    expect(url).toContain('labels=bug');
  });

  it('URL-encodes title + body safely', () => {
    const url = buildIssueUrl({
      kronnVersion: '0.4.0',
      hostOs: 'macOS',
      logLines: [],
    });
    // The raw brackets / spaces of the title must not appear verbatim —
    // that would mean the form value wasn't encoded. URLSearchParams uses
    // `+` for spaces (form encoding) and `%5B`/`%5D` for brackets.
    expect(url).not.toMatch(/title=\[Bug\] Kronn/);
    // Round-tripping the URL must recover the original title.
    const parsed = new URL(url);
    expect(parsed.searchParams.get('title')).toBe('[Bug] Kronn v0.4.0 on macOS');
  });

  it('trims log lines to stay under the URL length budget', () => {
    // 1000 long lines would blow past GitHub's URL cap if naively included.
    const longLine = 'x'.repeat(200);
    const lines = Array.from({ length: 1000 }, () => longLine);
    const url = buildIssueUrl({ logLines: lines });
    // Must not exceed the budget — concrete assertion guards against a
    // future refactor that drops the cap.
    expect(url.length).toBeLessThanOrEqual(6000);
  });
});
