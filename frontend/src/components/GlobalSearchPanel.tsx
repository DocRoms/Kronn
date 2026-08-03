// KT-65 — search every message, from the sidebar.
//
// Deliberately self-contained: it owns its query, filters and paging, and hands
// a chosen result back through `onOpenResult`. The parent decides what "open"
// means (select the discussion, scroll to the message), which keeps this
// component testable without the whole discussions page around it.
//
// Nothing is searched client-side: the backend clamps the limit and caps the
// offset, so a long history costs one bounded request per page instead of
// shipping every conversation to the browser.

import { useCallback, useEffect, useRef, useState } from 'react';
import { discussions as discussionsApi } from '../lib/api';
import { formatRelativeTime } from '../lib/relativeTime';
import { Search, X, Loader2 } from 'lucide-react';
import type { MessageSearchHit, Project } from '../types/generated';

const PAGE_SIZE = 20;

/** The excerpt is centred on the match, but the eye still has to find it in a
 *  wall of text — so mark every occurrence instead of leaving the reader to
 *  scan. Split on the term rather than injecting HTML: a query is user input
 *  and must never reach the DOM as markup. */
function highlightTerm(snippet: string, term: string): React.ReactNode[] {
  const needle = term.trim();
  if (!needle) return [snippet];
  const lowerSnippet = snippet.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  let hit = lowerSnippet.indexOf(lowerNeedle);
  while (hit !== -1) {
    if (hit > cursor) nodes.push(snippet.slice(cursor, hit));
    // Keep the ORIGINAL casing of the match, not the query's.
    nodes.push(<mark key={`${hit}-${nodes.length}`}>{snippet.slice(hit, hit + needle.length)}</mark>);
    cursor = hit + needle.length;
    hit = lowerSnippet.indexOf(lowerNeedle, cursor);
  }
  if (cursor < snippet.length) nodes.push(snippet.slice(cursor));
  return nodes;
}

export interface GlobalSearchPanelProps {
  projects: Project[];
  /** Authors offered in the filter — agent types and known human pseudos. */
  authors: string[];
  /** Reuse the sidebar's quick-search value when advanced search opens so
   *  switching modes never forces the user to type the same query twice. */
  initialQuery?: string;
  /** Keep the shared sidebar query in sync while the advanced panel is open. */
  onQueryChange?: (query: string) => void;
  onOpenResult: (hit: MessageSearchHit) => void;
  onClose: () => void;
  t: (key: string, ...args: (string | number)[]) => string;
  lang?: string;
}

export function GlobalSearchPanel({
  projects,
  authors,
  initialQuery = '',
  onQueryChange,
  onOpenResult,
  onClose,
  t,
  lang = 'fr',
}: GlobalSearchPanelProps) {
  const [query, setQuery] = useState(initialQuery);
  const [projectId, setProjectId] = useState('');
  const [author, setAuthor] = useState('');
  const [since, setSince] = useState('');
  const [until, setUntil] = useState('');
  const [hits, setHits] = useState<MessageSearchHit[]>([]);
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exhausted, setExhausted] = useState(false);
  const [searched, setSearched] = useState(false);
  // The term the displayed hits came from. Highlighting the live input
  // instead would make the marks flicker while the next query is typed.
  const [submittedQuery, setSubmittedQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  // Guards against a slow first page landing after a newer search started.
  const runIdRef = useRef(0);
  // Opening the global panel from the sidebar with Enter must perform the
  // search the user just asked for, not force a second Enter on an identical
  // field. A ref keeps later filter/query edits explicit.
  const initialSearchStartedRef = useRef(false);

  useEffect(() => { inputRef.current?.focus(); }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const run = useCallback(async (nextOffset: number) => {
    const trimmed = query.trim();
    if (!trimmed) return;
    const runId = ++runIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const page = await discussionsApi.searchMessages({
        q: trimmed,
        projectId: projectId || undefined,
        author: author || undefined,
        // A date input gives a bare day; the backend compares RFC3339 strings,
        // so widen to cover the whole day rather than dropping matches.
        since: since ? `${since}T00:00:00Z` : undefined,
        until: until ? `${until}T23:59:59Z` : undefined,
        limit: PAGE_SIZE,
        offset: nextOffset,
      });
      if (runId !== runIdRef.current) return;
      setSubmittedQuery(trimmed);
      setHits(previous => (nextOffset === 0 ? page : [...previous, ...page]));
      setOffset(nextOffset + page.length);
      setExhausted(page.length < PAGE_SIZE);
      setSearched(true);
    } catch (e) {
      if (runId !== runIdRef.current) return;
      setError(String(e));
    } finally {
      if (runId === runIdRef.current) setLoading(false);
    }
  }, [query, projectId, author, since, until]);

  useEffect(() => {
    if (initialSearchStartedRef.current || !initialQuery.trim()) return;
    initialSearchStartedRef.current = true;
    void run(0);
  }, [initialQuery, run]);

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setHits([]);
    setOffset(0);
    setExhausted(false);
    run(0);
  };

  return (
    <aside className="disc-global-search" data-testid="global-search-panel" aria-label={t('disc.globalSearch.title')}>
      <header className="disc-global-search-header">
        <h3><Search size={13} /> {t('disc.globalSearch.title')}</h3>
        <button
          type="button"
          className="disc-global-search-close"
          onClick={onClose}
          aria-label={t('disc.globalSearch.close')}
        >
          <X size={14} />
        </button>
      </header>

      <form className="disc-global-search-form" onSubmit={submit}>
        <input
          ref={inputRef}
          className="input disc-global-search-input"
          value={query}
          onChange={event => {
            setQuery(event.target.value);
            onQueryChange?.(event.target.value);
          }}
          placeholder={t('disc.globalSearch.placeholder')}
          data-testid="global-search-input"
        />
        <div className="disc-global-search-filters">
          <select
            value={projectId}
            onChange={event => setProjectId(event.target.value)}
            aria-label={t('disc.globalSearch.filterProject')}
            data-testid="global-search-project"
          >
            <option value="">{t('disc.globalSearch.anyProject')}</option>
            {projects.map(project => (
              <option key={project.id} value={project.id}>{project.name}</option>
            ))}
          </select>
          <select
            value={author}
            onChange={event => setAuthor(event.target.value)}
            aria-label={t('disc.globalSearch.filterAuthor')}
            data-testid="global-search-author"
          >
            <option value="">{t('disc.globalSearch.anyAuthor')}</option>
            {authors.map(name => <option key={name} value={name}>{name}</option>)}
          </select>
          <input
            type="date"
            value={since}
            onChange={event => setSince(event.target.value)}
            aria-label={t('disc.globalSearch.filterSince')}
            data-testid="global-search-since"
          />
          <input
            type="date"
            value={until}
            onChange={event => setUntil(event.target.value)}
            aria-label={t('disc.globalSearch.filterUntil')}
            data-testid="global-search-until"
          />
        </div>
        <button type="submit" className="disc-global-search-submit" disabled={!query.trim() || loading}>
          {loading ? <Loader2 size={12} className="spin" /> : <Search size={12} />}
          {t('disc.globalSearch.run')}
        </button>
      </form>

      {error && <p className="disc-global-search-error">{t('disc.globalSearch.failed', error)}</p>}

      <div className="disc-global-search-results">
        {hits.map(hit => (
          <button
            type="button"
            key={hit.message_id}
            className="disc-global-search-hit"
            onClick={() => onOpenResult(hit)}
            data-tour-id="global-search-result"
            data-disc-id={hit.disc_id}
          >
            <span className="disc-global-search-hit-meta">
              <strong>{hit.disc_title}</strong>
              <span>{hit.agent_type ?? hit.author_pseudo ?? hit.role}</span>
              <span>{formatRelativeTime(hit.timestamp, lang)}</span>
            </span>
            <span className="disc-global-search-hit-snippet">
              {highlightTerm(hit.snippet, submittedQuery)}
            </span>
          </button>
        ))}
        {searched && hits.length === 0 && !loading && (
          <p className="disc-global-search-empty">{t('disc.globalSearch.noResult')}</p>
        )}
        {hits.length > 0 && !exhausted && (
          <button
            type="button"
            className="disc-global-search-more"
            onClick={() => run(offset)}
            disabled={loading}
            data-testid="global-search-more"
          >
            {t('disc.globalSearch.more')}
          </button>
        )}
      </div>
    </aside>
  );
}
