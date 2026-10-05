import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

vi.mock('../../../lib/api', () => buildApiMock());
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join(',')}` : key,
    locale: 'en',
    setLocale: () => {},
  }),
}));

import { workflows as workflowsApi } from '../../../lib/api';
import { ExecScriptFilesEditor } from '../ExecScriptFilesEditor';
import { parseScriptPaths } from '../execScriptPaths';

const HASH = 'a'.repeat(64);

describe('ExecScriptFilesEditor (KT-918)', () => {
  beforeEach(() => vi.mocked(workflowsApi.execScriptStatus).mockReset());

  it('keeps the approved hash of a path that stays and leaves a new one to approve at save', () => {
    expect(parseScriptPaths('a.cjs\n  lib/b.cjs \n\na.cjs\n', [{ path: 'a.cjs', sha256: HASH }])).toEqual([
      { path: 'a.cjs', sha256: HASH },
      { path: 'lib/b.cjs', sha256: '' },
    ]);
    expect(parseScriptPaths('', [{ path: 'a.cjs', sha256: HASH }])).toEqual([]);
  });

  it('shows each file state and re-approves only the changed file', async () => {
    vi.mocked(workflowsApi.execScriptStatus).mockResolvedValue([
      { path: 'scripts/jira.cjs', state: 'approved', current_sha256: HASH },
      { path: 'lib/http.cjs', state: 'changed', current_sha256: 'b'.repeat(64) },
    ]);
    const files = [
      { path: 'scripts/jira.cjs', sha256: HASH },
      { path: 'lib/http.cjs', sha256: 'c'.repeat(64) },
    ];
    const onChange = vi.fn();
    render(<ExecScriptFilesEditor files={files} projectId="proj-1" onChange={onChange} />);

    expect(await screen.findByText('wiz.execScriptState.changed')).toBeInTheDocument();
    expect(screen.getByText('wiz.execScriptState.approved')).toBeInTheDocument();
    expect(screen.getByText('wiz.execScriptsChangedWarn:lib/http.cjs')).toBeInTheDocument();
    expect(workflowsApi.execScriptStatus).toHaveBeenCalledWith({ project_id: 'proj-1', files });

    fireEvent.click(screen.getByRole('button', { name: 'wiz.execScriptsApprove' }));
    expect(onChange).toHaveBeenCalledWith([
      { path: 'scripts/jira.cjs', sha256: HASH },
      { path: 'lib/http.cjs', sha256: '' },
    ]);
  });

  it('asks for a project and never queries without one', async () => {
    render(
      <ExecScriptFilesEditor files={[{ path: 'a.cjs', sha256: '' }]} projectId="" onChange={vi.fn()} />,
    );
    expect(screen.getByText('wiz.execScriptsNoProject')).toBeInTheDocument();
    await new Promise(resolve => setTimeout(resolve, 350));
    expect(workflowsApi.execScriptStatus).not.toHaveBeenCalled();
  });

  it('turns typed lines into declared files', () => {
    const onChange = vi.fn();
    render(<ExecScriptFilesEditor files={[]} projectId="proj-1" onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('wiz.execScripts'), {
      target: { value: 'scripts/jira.cjs\nlib/http.cjs' },
    });
    expect(onChange).toHaveBeenLastCalledWith([
      { path: 'scripts/jira.cjs', sha256: '' },
      { path: 'lib/http.cjs', sha256: '' },
    ]);
  });
});
