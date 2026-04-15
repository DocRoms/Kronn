import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  saveAuditCheckpoint,
  loadAuditCheckpoint,
  clearAuditCheckpoint,
  AUDIT_CHECKPOINT_CONFIG,
  type AuditCheckpoint,
} from '../audit-resume';

const baseCp: AuditCheckpoint = {
  projectId: 'proj-1',
  kind: 'full',
  startedAt: '2026-04-15T09:00:00.000Z',
  stepIndex: 3,
  totalSteps: 10,
  currentFile: 'repo-map.md',
};

describe('audit-resume checkpoint', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-15T09:05:00Z'));
  });
  afterEach(() => { vi.useRealTimers(); });

  it('round-trips a checkpoint through localStorage', () => {
    saveAuditCheckpoint(baseCp);
    const got = loadAuditCheckpoint('proj-1');
    expect(got).toEqual(baseCp);
  });

  it('isolates checkpoints per project id', () => {
    saveAuditCheckpoint(baseCp);
    saveAuditCheckpoint({ ...baseCp, projectId: 'proj-2', stepIndex: 1, currentFile: 'decisions.md' });
    expect(loadAuditCheckpoint('proj-1')!.stepIndex).toBe(3);
    expect(loadAuditCheckpoint('proj-2')!.stepIndex).toBe(1);
  });

  it('returns null when no checkpoint exists', () => {
    expect(loadAuditCheckpoint('never-saved')).toBeNull();
  });

  it('clearAuditCheckpoint removes the entry', () => {
    saveAuditCheckpoint(baseCp);
    clearAuditCheckpoint('proj-1');
    expect(loadAuditCheckpoint('proj-1')).toBeNull();
  });

  it('ignores stale checkpoints older than 1 h and purges them on load', () => {
    saveAuditCheckpoint({ ...baseCp, startedAt: '2026-04-15T07:00:00.000Z' }); // 2 h old
    expect(loadAuditCheckpoint('proj-1')).toBeNull();
    expect(localStorage.getItem(AUDIT_CHECKPOINT_CONFIG.KEY_PREFIX + 'proj-1')).toBeNull();
  });

  it('accepts a fresh checkpoint at the TTL boundary', () => {
    const exactly1hAgo = new Date(Date.now() - AUDIT_CHECKPOINT_CONFIG.MAX_AGE_MS).toISOString();
    saveAuditCheckpoint({ ...baseCp, startedAt: exactly1hAgo });
    expect(loadAuditCheckpoint('proj-1')).not.toBeNull();
  });

  it('rejects malformed JSON', () => {
    localStorage.setItem(AUDIT_CHECKPOINT_CONFIG.KEY_PREFIX + 'proj-1', '{bad');
    expect(loadAuditCheckpoint('proj-1')).toBeNull();
  });

  it('rejects unknown schema version (forward-compat)', () => {
    localStorage.setItem(
      AUDIT_CHECKPOINT_CONFIG.KEY_PREFIX + 'proj-1',
      JSON.stringify({ v: 99, ...baseCp }),
    );
    expect(loadAuditCheckpoint('proj-1')).toBeNull();
  });

  it('rejects entries missing required fields', () => {
    localStorage.setItem(
      AUDIT_CHECKPOINT_CONFIG.KEY_PREFIX + 'proj-1',
      JSON.stringify({ v: 1, projectId: 'proj-1' }), // missing step/total/startedAt
    );
    expect(loadAuditCheckpoint('proj-1')).toBeNull();
  });

  it('handles empty project id safely', () => {
    expect(() => saveAuditCheckpoint({ ...baseCp, projectId: '' })).not.toThrow();
    expect(() => clearAuditCheckpoint('')).not.toThrow();
    expect(loadAuditCheckpoint('')).toBeNull();
  });
});
