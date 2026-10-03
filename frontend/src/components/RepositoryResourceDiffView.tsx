import { useMemo, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { parseUnifiedDiff, sideBySide } from '../lib/repositoryResourceDiff';

type DiffMode = 'unified' | 'side';

const DIFF_MARK = { added: '+', removed: '-', context: ' ', hunk: '' } as const;

function DiffView({ diff, mode }: { diff: string; mode: DiffMode }) {
  const lines = useMemo(() => parseUnifiedDiff(diff), [diff]);
  const rows = useMemo(() => sideBySide(lines), [lines]);
  if (mode === 'unified') {
    return (
      <pre className="rr-diff" data-mode="unified">
        {lines.map((line, index) => (
          <span key={index} data-line={line.kind}>
            {line.kind === 'hunk' ? line.text : `${DIFF_MARK[line.kind]}${line.text}`}{'\n'}
          </span>
        ))}
      </pre>
    );
  }
  return (
    <div className="rr-diff" data-mode="side">
      {rows.map((row, index) => (
        <div key={index} className="rr-diff-row" data-line={row.kind}>
          <code data-side="repository" data-empty={row.left === null || undefined}>{row.left ?? ''}</code>
          <code data-side="kronn" data-empty={row.right === null || undefined}>{row.right ?? ''}</code>
        </div>
      ))}
    </div>
  );
}

/** The text diff of every differing file, unified or side by side: the one
 *  component behind the Compare sheet and the "Diff" mode of a resource's sheet. */
export function DiffFiles({ diffs }: { diffs: Array<{ path: string; diff: string }> }) {
  const { t } = useT();
  const [mode, setMode] = useState<DiffMode>('unified');
  if (diffs.length === 0) return null;
  return (
    <>
      <div className="rr-diff-toolbar">
        <span>{t('projects.repositoryResources.compare.files', diffs.length)}</span>
        <div role="group" aria-label={t('projects.repositoryResources.compare.view')}>
          {(['unified', 'side'] as const).map(option => (
            <button
              key={option}
              type="button"
              className="rr-chip"
              aria-pressed={mode === option}
              onClick={() => setMode(option)}
            >
              {t(`projects.repositoryResources.compare.${option}`)}
            </button>
          ))}
        </div>
      </div>
      {diffs.map(file => (
        <div key={file.path} className="rr-diff-file">
          <code className="rr-diff-path">{file.path}</code>
          <DiffView diff={file.diff} mode={mode} />
        </div>
      ))}
    </>
  );
}
