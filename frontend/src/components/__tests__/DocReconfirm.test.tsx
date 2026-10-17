// « À reconfirmer » (KT-1079): hidden while nothing waits, lists the docs Kronn
// changed since a human validated them with their diff, and « Confirmer »
// gives the provenance back with a single request per click.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor, within, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { buildApiMock } from '../../test/apiMock';
import { expectFocusKeptIn } from '../../test/dialogFocus';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));

import { DocReconfirm } from '../DocReconfirm';
import { lineDiff } from '../../lib/lineDiff';
import { projects as projectsApi } from '../../lib/api';
import type { DocProvenanceReport, DocProvenanceStatus, Project } from '../../types/generated';

const PROJECT: Project = {
  id: 'p-doc',
  name: 'doc-target',
  path: '/repos/doc-target',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'Validated',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const doc = (path: string, overrides: Partial<DocProvenanceStatus> = {}): DocProvenanceStatus => ({
  doc: path,
  state: 'to_reconfirm',
  reason: 'audit_validation',
  scope: 'doc',
  validated_at: '2026-10-01T10:00:00Z',
  sections: [],
  kronn_writes: [{ writer: 'full_audit', at: '2026-10-02T10:00:00Z' }],
  changes: [{ section: null, before: '# Map\nline two\n', after: '# Map\nline two, corrected\n', truncated: false }],
  ...overrides,
});

const report = (docs: DocProvenanceStatus[]): DocProvenanceReport => ({
  docs,
  to_reconfirm: docs.filter(d => d.state === 'to_reconfirm').length,
});

const provenance = () => projectsApi.docProvenance as ReturnType<typeof vi.fn>;
const confirmApi = () => projectsApi.confirmDocProvenance as ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
});

describe('DocReconfirm', () => {
  it('renders nothing while no doc waits for a reconfirmation', async () => {
    provenance().mockResolvedValue(report([doc('docs/a.md', { state: 'human_validated' })]));
    const { container } = render(<DocReconfirm project={PROJECT} auditActive={false} toast={vi.fn()} />);
    await waitFor(() => expect(provenance()).toHaveBeenCalledWith('p-doc'));
    expect(container.innerHTML).toBe('');
  });

  it('stays hidden and fetches nothing during an audit', () => {
    render(<DocReconfirm project={PROJECT} auditActive toast={vi.fn()} />);
    expect(provenance()).not.toHaveBeenCalled();
    expect(screen.queryByTestId('project-doc-reconfirm')).toBeNull();
  });

  it('lists each doc to reconfirm with its writer and its diff', async () => {
    const pending = report([doc('docs/repo-map.md'), doc('docs/AGENTS.md', {
      scope: 'sections', sections: ['team'], kronn_writes: [],
      changes: [{ section: 'team', before: null, after: 'Ask Ana', truncated: false }],
    })]);
    provenance().mockResolvedValue(pending);
    render(<DocReconfirm project={PROJECT} auditActive={false} toast={vi.fn()} />);
    const button = await screen.findByTestId('project-doc-reconfirm');
    expect(button.textContent).toContain('docReconfirm.button 2');
    fireEvent.click(button);
    const panel = await screen.findByTestId('doc-reconfirm-panel');
    await waitFor(() => expect(within(panel).getAllByTestId('doc-reconfirm-doc')).toHaveLength(2));
    expect(provenance()).toHaveBeenLastCalledWith('p-doc', true);
    const map = within(panel).getAllByTestId('doc-reconfirm-doc')[0];
    expect(map.textContent).toContain('docReconfirm.writer.full_audit');
    expect(map.textContent).toContain('+ line two, corrected');
    expect(map.textContent).toContain('- line two');
    const agents = within(panel).getAllByTestId('doc-reconfirm-doc')[1];
    expect(agents.textContent).toContain('docReconfirm.writer.outside');
    expect(agents.textContent).toContain('docReconfirm.beforeUnavailable');
    expect(agents.textContent).toContain('docReconfirm.section team');
  });

  it('confirms one doc with a single request and hides once nothing is left', async () => {
    provenance().mockResolvedValue(report([doc('docs/repo-map.md')]));
    let release: (r: DocProvenanceReport) => void = () => {};
    confirmApi().mockReturnValueOnce(new Promise<DocProvenanceReport>(r => { release = r; }));
    const toast = vi.fn();
    render(<DocReconfirm project={PROJECT} auditActive={false} toast={toast} />);
    fireEvent.click(await screen.findByTestId('project-doc-reconfirm'));
    const confirm = await screen.findByTestId('doc-reconfirm-confirm');
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    expect(confirmApi()).toHaveBeenCalledTimes(1);
    expect(confirmApi()).toHaveBeenCalledWith('p-doc', ['docs/repo-map.md']);
    release(report([doc('docs/repo-map.md', { state: 'human_validated' })]));
    await waitFor(() => expect(screen.queryByTestId('project-doc-reconfirm')).toBeNull());
    expect(toast).toHaveBeenCalledWith('docReconfirm.done 1', 'success');
  });

  it('« Tout confirmer » sends an empty list', async () => {
    provenance().mockResolvedValue(report([doc('docs/a.md'), doc('docs/b.md')]));
    confirmApi().mockResolvedValue(report([]));
    render(<DocReconfirm project={PROJECT} auditActive={false} toast={vi.fn()} />);
    fireEvent.click(await screen.findByTestId('project-doc-reconfirm'));
    fireEvent.click(await screen.findByTestId('doc-reconfirm-all'));
    await waitFor(() => expect(confirmApi()).toHaveBeenCalledWith('p-doc', []));
  });

  it('reports a failed confirmation', async () => {
    provenance().mockResolvedValue(report([doc('docs/a.md')]));
    confirmApi().mockRejectedValue(new Error('lease held'));
    const toast = vi.fn();
    render(<DocReconfirm project={PROJECT} auditActive={false} toast={toast} />);
    fireEvent.click(await screen.findByTestId('project-doc-reconfirm'));
    fireEvent.click(await screen.findByTestId('doc-reconfirm-confirm'));
    await waitFor(() => expect(toast).toHaveBeenCalledWith('docReconfirm.error lease held', 'error'));
  });

  it('loads details when a controlled parent opens the panel and closes on a read error', async () => {
    provenance().mockImplementation((_id: string, detail = false) => detail
      ? Promise.reject(new Error('read failed'))
      : Promise.resolve(report([doc('docs/a.md')])));
    const toast = vi.fn();
    const onOpenChange = vi.fn();
    function Harness() {
      const [open, setOpen] = useState(true);
      return (
        <DocReconfirm
          project={PROJECT}
          auditActive={false}
          toast={toast}
          open={open}
          onOpenChange={next => { onOpenChange(next); setOpen(next); }}
          hideTrigger
        />
      );
    }
    render(<Harness />);

    await waitFor(() => expect(provenance()).toHaveBeenCalledWith('p-doc', true));
    expect(toast).toHaveBeenCalledWith('docReconfirm.error read failed', 'error');
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(screen.queryByTestId('doc-reconfirm-panel')).toBeNull();
  });

  it('cancels a controlled detail read when the parent closes the panel', async () => {
    let release: (value: DocProvenanceReport) => void = () => {};
    provenance().mockImplementation((_id: string, detail = false) => detail
      ? new Promise<DocProvenanceReport>(resolve => { release = resolve; })
      : Promise.resolve(report([doc('docs/a.md')])));
    const toast = vi.fn();
    const { rerender } = render(
      <DocReconfirm project={PROJECT} auditActive={false} toast={toast} open hideTrigger />,
    );
    expect(await screen.findByTestId('doc-reconfirm-panel')).toBeInTheDocument();

    rerender(<DocReconfirm project={PROJECT} auditActive={false} toast={toast} open={false} hideTrigger />);
    await act(async () => { release(report([doc('docs/late.md')])); await Promise.resolve(); });

    expect(screen.queryByTestId('doc-reconfirm-panel')).toBeNull();
    expect(screen.queryByText('docs/late.md')).toBeNull();
    expect(toast).not.toHaveBeenCalled();
  });
});

describe('lineDiff', () => {
  it('keeps two lines of context around the change', () => {
    const before = ['a', 'b', 'c', 'd', 'e', 'f'].join('\n');
    const after = ['a', 'b', 'c', 'X', 'e', 'f'].join('\n');
    expect(lineDiff(before, after)).toEqual([
      { kind: 'same', text: 'b' },
      { kind: 'same', text: 'c' },
      { kind: 'add', text: 'X' },
      { kind: 'del', text: 'd' },
      { kind: 'same', text: 'e' },
      { kind: 'same', text: 'f' },
    ]);
  });

  it('gives up on a huge middle instead of freezing the page', () => {
    const before = Array.from({ length: 1000 }, (_, i) => `old ${i}`).join('\n');
    const after = Array.from({ length: 1000 }, (_, i) => `new ${i}`).join('\n');
    expect(lineDiff(before, after)).toBeNull();
  });

  it('the panel takes focus, keeps it, closes on Escape and returns focus to its trigger (KT-1072)', async () => {
    provenance().mockResolvedValue(report([doc('docs/a.md'), doc('docs/b.md')]));
    render(<DocReconfirm project={PROJECT} auditActive={false} toast={vi.fn()} />);
    const trigger = await screen.findByTestId('project-doc-reconfirm');
    await userEvent.click(trigger);
    const panel = await screen.findByTestId('doc-reconfirm-panel');
    await within(panel).findAllByTestId('doc-reconfirm-doc');
    await expectFocusKeptIn(panel);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByTestId('doc-reconfirm-panel')).toBeNull();
    expect(trigger).toHaveFocus();
  });
});
