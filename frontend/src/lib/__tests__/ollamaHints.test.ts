import { describe, it, expect } from 'vitest';
import { promptNeedsFileAccess } from '../ollamaHints';

describe('promptNeedsFileAccess', () => {
  it('flags prompts that rely on reading files / the worktree', () => {
    for (const p of [
      'Review la PR en lisant le fichier .review-context.json',
      'git -C <worktreePath> diff main...HEAD',
      'Lis le worktree et ouvre les fichiers à risque',
      'read the files in the repo',
      'Ouvre les fichiers des hunks à risque',
      'Consulte le diff dans .kronn/current_task.json',
    ]) {
      expect(promptNeedsFileAccess(p)).toBe(true);
    }
  });

  it('does NOT flag self-contained prompts (safe for a tool-less local model)', () => {
    for (const p of [
      'Classe ce ticket en un mot: bug ou feature',
      'Résume ce texte en une phrase',
      'Extrais le sentiment en JSON, réponds juste OK',
      '',
    ]) {
      expect(promptNeedsFileAccess(p)).toBe(false);
    }
  });

  it('handles null/undefined', () => {
    expect(promptNeedsFileAccess(null)).toBe(false);
    expect(promptNeedsFileAccess(undefined)).toBe(false);
  });
});
