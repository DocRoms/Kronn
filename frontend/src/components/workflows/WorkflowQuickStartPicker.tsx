// 0.8.5 — single discoverable entry point for "start from a template/
// suggestion/preset" in the workflow wizard.
//
// Replaces three previous separate UI sections (STARTER_TEMPLATES
// buttons, project-suggestions panel, v0.7 preset bandeau) with one
// collapsible picker pinned at the top of the wizard's step 0, right
// under the workflow name input. The collapsed state shows a single
// CTA chip with the total count; expanded shows a searchable, sortable,
// source-filterable list. One click on any row routes the unified
// payload back to the wizard which knows how to apply each source.

import { useEffect, useMemo, useState } from 'react';
import { ChevronRight, Sparkles, Search, X } from 'lucide-react';
import type {
  UnifiedQuickStart,
  QuickStartSource,
  QuickStartComplexity,
} from '../../lib/workflow-quick-start';
import { filterQuickStart, quickStartStepsPreview } from '../../lib/workflow-quick-start';

export interface WorkflowQuickStartPickerProps {
  /** Pre-built catalogue from `buildQuickStartCatalogue`. */
  entries: UnifiedQuickStart[];
  /** Whether the wizard is freshly opened (true) or editing an existing
   *  workflow (false). On edit we hide the picker entirely — the user
   *  has already committed to a config. */
  isEdit: boolean;
  /** Whether suggestions are still being fetched in the background. */
  loading?: boolean;
  /** Gate the picker until the wizard's prerequisites are met. Pre-fix
   *  the user could click a template before naming the workflow and
   *  the validator bounced them back to step 0 — confusing because
   *  the picker sat at the top with no visible precondition. Now we
   *  surface the gate (grey state + tooltip) so the user knows what
   *  to do first. The wizard owns the rule (currently "name must be
   *  non-empty"); the picker is purely presentational. */
  disabled?: boolean;
  /** One-line explanation rendered as the toggle's `title` attribute
   *  when `disabled` is true. Localised by the caller. */
  disabledReason?: string;
  /** Triggered when the user clicks "Use this" on a row. The wizard
   *  branches on `entry.payload.kind` to call the right setters. */
  onApply: (entry: UnifiedQuickStart) => void;
  /** Translator from the i18n context. */
  t: (key: string, ...args: (string | number)[]) => string;
}

const ALL_SOURCES: QuickStartSource[] = ['preset', 'starter', 'project-suggestion'];
const ALL_COMPLEXITIES: QuickStartComplexity[] = ['simple', 'intermediate', 'advanced'];

export function WorkflowQuickStartPicker({
  entries,
  isEdit,
  loading,
  disabled,
  disabledReason,
  onApply,
  t,
}: WorkflowQuickStartPickerProps) {
  // Picker is hidden during edit — the user already committed. Avoids
  // accidentally clobbering a careful manual config with one stray click.
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  // Source + complexity filters — empty array = no filtering on that axis.
  const [sourceFilter, setSourceFilter] = useState<QuickStartSource[]>([]);
  const [complexityFilter, setComplexityFilter] = useState<QuickStartComplexity[]>([]);

  // 0.8.5 dogfooding follow-up — if the wizard turns the gate ON while
  // the panel is expanded (e.g. the user cleared the name field), we
  // collapse so the disabled state is unambiguous. Re-opens on next
  // user click after the gate releases. useEffect (not inline setState
  // during render) per the React rules-of-hooks: derived setState in
  // render works but warns on cross-component updates.
  useEffect(() => {
    if (disabled && open) setOpen(false);
  }, [disabled, open]);

  const filtered = useMemo(() => {
    let list = entries;
    if (sourceFilter.length > 0) {
      list = list.filter(e => sourceFilter.includes(e.source));
    }
    if (complexityFilter.length > 0) {
      list = list.filter(e => complexityFilter.includes(e.complexity));
    }
    return filterQuickStart(list, query);
  }, [entries, sourceFilter, complexityFilter, query]);

  if (isEdit || entries.length === 0) return null;

  // Collapsed state — single CTA chip, count of entries, click to expand.
  if (!open) {
    return (
      <button
        type="button"
        className="wf-quickstart-toggle"
        onClick={() => setOpen(true)}
        aria-expanded="false"
        aria-disabled={disabled || undefined}
        disabled={disabled}
        title={disabled ? disabledReason : undefined}
      >
        <Sparkles size={12} />
        <span>{t('wiz.quickstart.toggle', entries.length)}</span>
        <ChevronRight size={10} className="wf-chevron" />
      </button>
    );
  }

  return (
    <section className="wf-quickstart-panel" aria-label={t('wiz.quickstart.title')}>
      <header className="wf-quickstart-header">
        <Sparkles size={14} className="text-accent" />
        <span className="wf-quickstart-title">{t('wiz.quickstart.title')}</span>
        <span className="wf-quickstart-hint">{t('wiz.quickstart.hint')}</span>
        <button
          type="button"
          className="mcp-icon-btn"
          onClick={() => setOpen(false)}
          aria-label={t('wiz.quickstart.collapse')}
          style={{ marginLeft: 'auto' }}
        >
          <X size={12} />
        </button>
      </header>

      <div className="wf-quickstart-controls">
        <div className="wf-quickstart-search">
          <Search size={11} className="wf-quickstart-search-icon" aria-hidden="true" />
          <input
            type="text"
            value={query}
            onChange={e => setQuery(e.target.value)}
            placeholder={t('wiz.quickstart.searchPlaceholder')}
            className="wf-input wf-quickstart-search-input"
            aria-label={t('wiz.quickstart.searchPlaceholder')}
          />
        </div>
        <div className="wf-quickstart-filters">
          <span className="text-xs text-ghost">{t('wiz.quickstart.filterComplexity')}</span>
          {ALL_COMPLEXITIES.map(c => {
            const active = complexityFilter.includes(c);
            return (
              <button
                key={c}
                type="button"
                className="wf-quickstart-filter-chip"
                data-active={active}
                onClick={() =>
                  setComplexityFilter(prev =>
                    active ? prev.filter(x => x !== c) : [...prev, c],
                  )
                }
              >
                {t(`wiz.quickstart.complexity.${c}`)}
              </button>
            );
          })}
        </div>
        <div className="wf-quickstart-filters">
          <span className="text-xs text-ghost">{t('wiz.quickstart.filterSource')}</span>
          {ALL_SOURCES.map(s => {
            const active = sourceFilter.includes(s);
            return (
              <button
                key={s}
                type="button"
                className="wf-quickstart-filter-chip"
                data-active={active}
                onClick={() =>
                  setSourceFilter(prev =>
                    active ? prev.filter(x => x !== s) : [...prev, s],
                  )
                }
              >
                {t(`wiz.quickstart.source.${s}`)}
              </button>
            );
          })}
        </div>
      </div>

      <ul className="wf-quickstart-list" role="list">
        {filtered.map(entry => (
          <QuickStartRow key={entry.id} entry={entry} onApply={onApply} t={t} />
        ))}
        {filtered.length === 0 && (
          <li className="wf-quickstart-empty">{t('wiz.quickstart.empty')}</li>
        )}
      </ul>

      {loading && (
        <div className="wf-quickstart-loading text-xs text-ghost" role="status">
          {t('wiz.quickstart.loading')}
        </div>
      )}
    </section>
  );
}

interface QuickStartRowProps {
  entry: UnifiedQuickStart;
  onApply: (entry: UnifiedQuickStart) => void;
  t: WorkflowQuickStartPickerProps['t'];
}

function QuickStartRow({ entry, onApply, t }: QuickStartRowProps) {
  const stepNames = quickStartStepsPreview(entry, 4);
  return (
    <li
      className="wf-quickstart-row"
      data-applicable={entry.applicable}
      data-source={entry.source}
      data-complexity={entry.complexity}
    >
      <div className="wf-quickstart-row-top">
        <span className="wf-quickstart-row-title">{entry.title}</span>
        <div className="wf-quickstart-row-badges">
          <span
            className="wf-quickstart-complexity-badge"
            data-complexity={entry.complexity}
            title={t('wiz.quickstart.complexityTooltip')}
          >
            {t(`wiz.quickstart.complexity.${entry.complexity}`)}
          </span>
          <span
            className="wf-quickstart-source-badge"
            data-source={entry.source}
            title={t('wiz.quickstart.sourceTooltip')}
          >
            {t(`wiz.quickstart.source.${entry.source}`)}
          </span>
          {entry.audience && (
            <span className="wf-quickstart-audience-badge">{entry.audience}</span>
          )}
        </div>
      </div>

      <p className="wf-quickstart-row-desc">{entry.description}</p>

      {entry.reason && (
        <p className="wf-quickstart-row-reason">{entry.reason}</p>
      )}

      <div className="wf-quickstart-row-meta">
        <span className="wf-quickstart-steps-count">
          {t('wiz.quickstart.stepsCount', entry.stepsCount)}
        </span>
        {entry.badges.map(b => (
          <span key={b} className="wf-quickstart-row-tag">{b}</span>
        ))}
        {stepNames.length > 0 && (
          <span className="wf-quickstart-steps-preview" aria-hidden="true">
            {stepNames.join(' › ')}
          </span>
        )}
      </div>

      {!entry.applicable && entry.notApplicableReason && (
        <p className="wf-quickstart-row-warning">
          {t('wiz.quickstart.notApplicable', entry.notApplicableReason)}
        </p>
      )}

      <div className="wf-quickstart-row-actions">
        <button
          type="button"
          className="wf-quickstart-apply-btn"
          onClick={() => onApply(entry)}
          disabled={!entry.applicable}
        >
          {t('wiz.quickstart.apply')}
        </button>
      </div>
    </li>
  );
}
