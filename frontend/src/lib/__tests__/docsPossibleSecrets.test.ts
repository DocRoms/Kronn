// KT-1796 — the possible secrets a step's document audit reported are read
// from the run state, and a damaged record is still said.

import { describe, it, expect } from 'vitest';
import type { WorkflowRun } from '../../types/generated';
import { runPossibleSecrets } from '../docsPossibleSecrets';

const run = (state: Record<string, string>) => ({ state }) as unknown as WorkflowRun;

describe('runPossibleSecrets', () => {
  it('reads documents and lines per step', () => {
    const report = JSON.stringify({
      documents: [{ path: 'docs/notes.md', lines: [2, 4], more_lines: 3 }],
      more_documents: 1,
    });
    expect(runPossibleSecrets(run({
      '__kronn.docs_possible_secrets.write.docs': report,
      'docs_write_rejections.write.docs': '1',
    }))).toEqual([{
      stepName: 'write.docs',
      documents: [{ path: 'docs/notes.md', lines: [2, 4], moreLines: 3 }],
      moreDocuments: 1,
      unreadable: false,
    }]);
  });

  it('keeps a record it cannot read', () => {
    for (const raw of ['not json', '{}', '{"documents":[]}', '{"documents":[{"path":1}]}']) {
      expect(runPossibleSecrets(run({ '__kronn.docs_possible_secrets.advise': raw }))).toEqual([
        { stepName: 'advise', documents: [], moreDocuments: 0, unreadable: true },
      ]);
    }
  });

  it('reads a record that only counts documents', () => {
    expect(runPossibleSecrets(run({ '__kronn.docs_possible_secrets.advise': '{"documents":[],"more_documents":4}' }))).toEqual([
      { stepName: 'advise', documents: [], moreDocuments: 4, unreadable: false },
    ]);
  });

  it('says nothing for a run without any', () => {
    expect(runPossibleSecrets(run({ other: 'x' }))).toEqual([]);
    expect(runPossibleSecrets({} as WorkflowRun)).toEqual([]);
  });

  it('reads the records adopted from child runs, an unread one included', () => {
    const report = JSON.stringify({ documents: [{ path: 'docs/a.md', lines: [4], more_lines: 0 }], more_documents: 0 });
    const found = runPossibleSecrets(run({
      '__kronn.docs_child_possible_secrets.write (sub-run aaaaaaaa)': report,
      '__kronn.docs_child_possible_secrets.fan-out': 'unread',
    }));
    expect(found.map(step => [step.stepName, step.unreadable])).toEqual([
      ['fan-out', true],
      ['write (sub-run aaaaaaaa)', false],
    ]);
  });
});
