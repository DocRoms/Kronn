import { useState } from 'react';
import { Loader2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { writtenPaths } from '../lib/repositoryResourceEffects';
import { linkedToggleNote, toggleLinked, type LinkNote, type RowGraph } from '../lib/repositoryResourceLinks';
import type { AlignDirection, AlignLine } from '../lib/repositoryResourceRows';
import { RepositoryResourceModal } from './RepositoryResourceModal';

interface Props {
  lines: AlignLine[];
  /** Every row, to follow the references between resources. */
  graph: RowGraph;
  /** Rows left out because they hold two versions or wait for approval. */
  excludedCount: number;
  canWrite: boolean;
  kronnExists: boolean;
  busy: boolean;
  onConfirm: (lines: AlignLine[]) => void;
  onCancel: () => void;
}

const SECTIONS: AlignDirection[] = ['to_kronn', 'to_repository'];

/** Recap of everything "Align all" would move, one checkbox per line, all
 *  ticked when it opens except what the repository's state forbids writing. */
export function RepositoryResourceAlign({
  lines, graph, excludedCount, canWrite, kronnExists, busy, onConfirm, onCancel,
}: Props) {
  const { t } = useT();
  const usable = (line: AlignLine) => line.direction === 'to_kronn' || canWrite;
  const [checked, setChecked] = useState(
    () => new Set(lines.filter(usable).map(line => line.row.key)),
  );
  const [note, setNote] = useState<LinkNote | null>(null);
  const chosen = lines.filter(line => checked.has(line.row.key) && usable(line));
  // Ticking a line ticks what it needs; a line something ticked still needs
  // stays ticked, and says why.
  const alignable = new Set(lines.filter(usable).map(line => line.row.key));
  const toggle = (key: string) => {
    const result = toggleLinked(graph, checked, key, dep => alignable.has(dep));
    setChecked(result.checked);
    setNote(linkedToggleNote(t, graph, key, result));
  };
  const repositoryRows = chosen.filter(line => line.direction === 'to_repository').map(line => line.row);

  return (
    <RepositoryResourceModal
      size="dialog"
      testId="repository-align"
      title={t('projects.repositoryResources.align.title')}
      subtitle={excludedCount > 0 ? t('projects.repositoryResources.align.excluded', excludedCount) : undefined}
      onClose={onCancel}
      footer={<>
        <button type="button" className="rr-button" onClick={onCancel}>{t('common.cancel')}</button>
        <button
          type="button"
          className="rr-button"
          data-tone="primary"
          disabled={busy || chosen.length === 0}
          onClick={() => onConfirm(chosen)}
        >
          {busy && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
          {t('projects.repositoryResources.align.confirm', chosen.length)}
        </button>
      </>}
    >
      {note && (
        <p className="rr-link-note" data-tone={note.tone} role="status" data-testid="align-link-note">{note.text}</p>
      )}
      {SECTIONS.map(direction => {
        const section = lines.filter(line => line.direction === direction);
        if (section.length === 0) return null;
        const disabled = direction === 'to_repository' && !canWrite;
        return (
          <section key={direction} className="rr-section" data-direction={direction}>
            <h3>{t(`projects.repositoryResources.align.${direction}`)}</h3>
            <p className="rr-muted">
              {direction === 'to_kronn'
                ? t('projects.repositoryResources.align.effectKronn')
                : disabled
                  ? t('projects.repositoryResources.banner.writeDisabled.title')
                  : t('projects.repositoryResources.align.effectRepository', writtenPaths(repositoryRows).length)}
              {direction === 'to_repository' && !kronnExists && !disabled && ` ${t('projects.repositoryResources.effect.createsFolder')}`}
            </p>
            <ul className="rr-align-list">
              {section.map(line => (
                <li key={line.row.key}>
                  <label>
                    <input
                      type="checkbox"
                      checked={checked.has(line.row.key) && usable(line)}
                      disabled={disabled || busy}
                      onChange={() => toggle(line.row.key)}
                    />
                    <span className="rr-align-name">{line.row.name}</span>
                    <code title={line.row.displayPath}>{line.row.displayPath}</code>
                  </label>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </RepositoryResourceModal>
  );
}
