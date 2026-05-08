import { describe, it, expect } from 'vitest';
import { userError } from '../userError';

describe('userError', () => {
  describe('passes backend ApiResponse strings through', () => {
    it('keeps "Project path" prefix verbatim', () => {
      expect(userError('Project path not found: /home/foo')).toBe(
        'Project path not found: /home/foo',
      );
    });

    it('keeps "Directory does not exist" verbatim', () => {
      expect(userError('Directory does not exist: /var/empty')).toBe(
        'Directory does not exist: /var/empty',
      );
    });

    it('keeps "Invalid" prefix verbatim (path traversal, branch name, etc.)', () => {
      expect(userError('Invalid branch name')).toBe('Invalid branch name');
      expect(userError('Invalid path')).toBe('Invalid path');
    });

    it('keeps "partial_pending" sentinel verbatim — the chat path matches it as a substring', () => {
      // `discussionsApi.sendMessageStream`'s onError checks for this exact
      // sentinel. If userError reformats it, the partial-recovery prompt
      // never fires and the user is stuck.
      const sentinel = 'partial_pending';
      expect(userError(sentinel)).toBe(sentinel);
    });

    it('keeps "Workflow" prefix verbatim', () => {
      expect(userError('Workflow not found')).toBe('Workflow not found');
    });
  });

  describe('translates network failures', () => {
    it('"Failed to fetch" → connection-lost message', () => {
      const out = userError('TypeError: Failed to fetch');
      expect(out.toLowerCase()).toContain('serveur');
    });

    it('"NetworkError" → connection-lost message', () => {
      const out = userError(new Error('NetworkError when attempting to fetch resource.'));
      expect(out.toLowerCase()).toContain('serveur');
    });

    it('"timeout" → too-long message', () => {
      const out = userError('Request timeout after 30s');
      expect(out.toLowerCase()).toMatch(/temps|timeout|réessayez|retry/i);
    });

    it('413 / "too large" → file-too-large message', () => {
      const out = userError('413: too large');
      expect(out.toLowerCase()).toMatch(/volumineux|too large/i);
    });

    it('401 / "Unauthorized" → expired-session message', () => {
      const out = userError('401 Unauthorized');
      expect(out.toLowerCase()).toMatch(/session|expir|recharger/i);
    });
  });

  describe('falls back to generic for gibberish', () => {
    it('returns fallback for stack traces (multi-line, contains "at ")', () => {
      const trace = 'TypeError: Cannot read property\n    at Object.<anonymous> (file.js:1:1)';
      const out = userError(trace);
      expect(out).not.toContain('at ');
      // Generic fallback (not the trace text)
      expect(out).not.toContain('TypeError');
    });

    it('returns fallback for very long messages', () => {
      const long = 'x'.repeat(500);
      const out = userError(long);
      expect(out).not.toBe(long);
    });

    it('uses caller-supplied fallback over the generic default', () => {
      expect(userError('TypeError: foo\n    at bar.js:1:1', 'Custom fallback')).toBe(
        'Custom fallback',
      );
    });

    it('returns the generic message when no error is supplied', () => {
      expect(userError(null)).not.toBe('');
      expect(userError(undefined)).not.toBe('');
      expect(userError('')).not.toBe('');
    });
  });

  describe('extracts message from any error shape', () => {
    it('Error instance', () => {
      expect(userError(new Error('Project path missing'))).toBe('Project path missing');
    });

    it('object with `message` property', () => {
      expect(userError({ message: 'Workflow not found' })).toBe('Workflow not found');
    });

    it('object with `error` property (some axios-style payloads)', () => {
      expect(userError({ error: 'Invalid token' })).toBe('Invalid token');
    });

    it('plain string', () => {
      expect(userError('Discussion not found')).toBe('Discussion not found');
    });
  });

  describe('keeps short backend-style messages even without a known prefix', () => {
    it('"DB error: column missing" passes through (short, no stack trace)', () => {
      // Not in BACKEND_PREFIXES, but short + no newline + no "at " marker.
      expect(userError('DB error: column missing')).toBe('DB error: column missing');
    });
  });
});
