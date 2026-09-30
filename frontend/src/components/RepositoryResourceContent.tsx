import { useId, useMemo, useState } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useT } from '../lib/I18nContext';
import {
  CONTENT_MODES,
  contentModes,
  filesOn,
  isMarkdownContent,
  splitFrontMatter,
  type ContentMode,
} from '../lib/repositoryResourceContent';
import type { ResourceRow } from '../lib/repositoryResourceRows';
import type { RepositoryResourceComparison } from '../types/generated';
import { DiffFiles } from './RepositoryResourceDiffView';

type TextView = 'rendered' | 'source';

const REMARK_PLUGINS = [remarkGfm];

/** Kept outside the component so a re-render does not remount the Markdown.
 *  A skill's links and images point into its own folder, which this page cannot
 *  serve: only a web link is followed, and an image is reduced to its caption. */
const MARKDOWN_COMPONENTS: Components = {
  a: ({ href, children }) => (
    href && /^https?:\/\//i.test(href)
      ? <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>
      : <span>{children}</span>
  ),
  img: ({ alt }) => <span>{alt}</span>,
};

function FileText({ text, markdown }: { text: string; markdown: boolean }) {
  if (!markdown) return <pre className="rr-content-text" data-testid="content-source">{text}</pre>;
  const { header, body } = splitFrontMatter(text);
  return (
    <div className="rr-markdown" data-testid="content-rendered">
      {header !== null && <pre className="rr-code">{header}</pre>}
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={MARKDOWN_COMPONENTS}>{body}</ReactMarkdown>
    </div>
  );
}

interface Props {
  row: ResourceRow;
  comparison: RepositoryResourceComparison;
  /** The text diffs of the files that differ, as the Compare sheet shows them. */
  diffs: Array<{ path: string; diff: string }>;
}

/** The resource itself, read in place: the file as the repository holds it, as
 *  Kronn holds it, or what differs. A mode with nothing to show is off, with the
 *  reason on hover; the sheet opens on the one that matters. */
export function RepositoryResourceContent({ row, comparison, diffs }: Props) {
  const { t } = useT();
  const reasonId = useId();
  const modes = useMemo(() => contentModes(row.state, comparison), [row.state, comparison]);
  // The pick is filed under the comparison it was made on: another resource, or
  // a reload of this one, opens on the preselected mode again.
  const [picked, setPicked] = useState<{ comparison: RepositoryResourceComparison; mode: ContentMode } | null>(null);
  const [view, setView] = useState<TextView>('rendered');
  const mode = picked?.comparison === comparison ? picked.mode : modes.preselected;
  const shown = mode === 'diff' ? [] : filesOn(comparison, mode);
  const markdownShown = shown.some(file => isMarkdownContent(row.kind, file.path));

  return (
    <section className="rr-section rr-content" data-testid="resource-content" data-mode={mode}>
      <h3>{t('projects.repositoryResources.content.title')}</h3>
      <div className="rr-content-modes" role="group" aria-label={t('projects.repositoryResources.content.modeLabel')}>
        {CONTENT_MODES.map(option => {
          const reason = modes.disabled[option];
          const reasonText = reason ? t(`projects.repositoryResources.content.disabled.${reason}`) : undefined;
          return (
            <span key={option} className="rr-content-mode" title={reasonText}>
              <button
                type="button"
                className="rr-chip"
                aria-pressed={mode === option}
                disabled={reason !== undefined}
                aria-describedby={reason ? `${reasonId}-${option}` : undefined}
                onClick={() => setPicked({ comparison, mode: option })}
              >
                {t(`projects.repositoryResources.content.mode.${option}`)}
              </button>
              {reason && <span id={`${reasonId}-${option}`} className="rr-visually-hidden">{reasonText}</span>}
            </span>
          );
        })}
      </div>
      {modes.newer && (
        <p className="rr-content-newer" data-side={modes.newer} data-testid="content-newer">
          {t(`projects.repositoryResources.content.newer.${modes.newer}`)}
        </p>
      )}
      {modes.disabled.diff === 'identical' && (
        <p className="rr-muted">{t('projects.repositoryResources.compare.noDiff')}</p>
      )}
      {(comparison.files ?? []).length === 0 && (
        <p className="rr-muted">{t('projects.repositoryResources.content.empty')}</p>
      )}
      {mode === 'diff' ? (
        <DiffFiles diffs={diffs} />
      ) : (
        <>
          {markdownShown && (
            <div className="rr-content-view" role="group" aria-label={t('projects.repositoryResources.content.viewLabel')}>
              {(['rendered', 'source'] as const).map(option => (
                <button
                  key={option}
                  type="button"
                  className="rr-chip"
                  aria-pressed={view === option}
                  onClick={() => setView(option)}
                >
                  {t(`projects.repositoryResources.content.${option}`)}
                </button>
              ))}
            </div>
          )}
          {shown.map(file => (
            <div key={file.path} className="rr-diff-file" data-testid="content-file">
              <code className="rr-diff-path">{file.path}</code>
              <FileText
                text={(mode === 'repository' ? file.repository : file.kronn) ?? ''}
                markdown={isMarkdownContent(row.kind, file.path) && view === 'rendered'}
              />
              {file.truncated && <p className="rr-muted">{t('projects.repositoryResources.content.truncated')}</p>}
            </div>
          ))}
        </>
      )}
    </section>
  );
}
