import { Loader2 } from 'lucide-react';
import { highlightLine, languageForPath, parseDiffLines } from '../lib/diff-syntax';

interface Props {
  path: string;
  content: string;
  loading: boolean;
  className?: string;
}

/** Shared, syntax-aware Git diff renderer.
 * Kept independent from GitPanel so Project > Code can reuse the exact same
 * visualisation when its Git-diff mode is added. */
export function GitDiffViewer({ path, content, loading, className = '' }: Props) {
  if (loading) {
    return (
      <div className={`git-diff-container ${className}`.trim()}>
        <div className="git-center">
          <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} />
        </div>
      </div>
    );
  }

  const language = languageForPath(path);
  const parsed = parseDiffLines(content);

  return (
    <div className={`git-diff-container ${className}`.trim()}>
      <pre className="git-diff-pre">
        {parsed.map((line, index) => {
          if (line.kind === 'del') {
            return (
              <div key={index} className="git-diff-line git-diff-line-del">
                <span className="git-diff-prefix">-</span>
                <span className="git-diff-content">{line.content || '\u00A0'}</span>
              </div>
            );
          }
          if (line.kind === 'hunk') {
            return (
              <div key={index} className="git-diff-line git-diff-line-hunk">
                <span className="git-diff-content">{line.raw}</span>
              </div>
            );
          }
          if (line.kind === 'meta') {
            return (
              <div key={index} className="git-diff-line git-diff-line-meta">
                <span className="git-diff-content">{line.raw || '\u00A0'}</span>
              </div>
            );
          }

          const prefix = line.kind === 'add' ? '+' : ' ';
          const kindClass = line.kind === 'add' ? 'git-diff-line-add' : 'git-diff-line-ctx';
          const html = highlightLine(line.content, language);
          return (
            <div key={index} className={`git-diff-line ${kindClass}`}>
              <span className="git-diff-prefix">{prefix}</span>
              <span
                className="git-diff-content hljs"
                dangerouslySetInnerHTML={{ __html: html || '\u00A0' }}
              />
            </div>
          );
        })}
      </pre>
    </div>
  );
}
