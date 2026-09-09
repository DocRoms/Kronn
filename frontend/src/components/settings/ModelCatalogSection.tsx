import { useEffect, useMemo, useState } from 'react';
import { ArrowDown, ArrowUp, Plus, RefreshCw, Search, Trash2, X } from 'lucide-react';
import { modelCatalogApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type {
  AgentType,
  CatalogModelEntry,
  ModelCatalogSnapshot,
  ModelCostHint,
  ModelTier,
} from '../../types/generated';

const PAGE_SIZE = 60;

type SortColumn = 'model' | 'target' | 'provenance' | 'availability' | 'cost';

interface CatalogRow {
  model: CatalogModelEntry;
  targetId: string;
  targetLabel: string;
}

interface ManualForm {
  runtimeTargetId: string;
  agentType: AgentType;
  modelId: string;
  displayName: string;
  capabilities: string[];
  reasoningModes: string;
  tier: ModelTier | '';
  costHint: ModelCostHint | '';
  privacyNote: string;
}

const blankForm = (snapshot: ModelCatalogSnapshot | null): ManualForm => {
  const target = snapshot?.targets[0];
  return {
    runtimeTargetId: target?.runtime_target_id ?? 'agent:claude-code',
    agentType: target?.agent_type ?? 'ClaudeCode',
    modelId: '',
    displayName: '',
    capabilities: ['chat'],
    reasoningModes: '',
    tier: '',
    costHint: '',
    privacyNote: '',
  };
};

export function ModelCatalogSection({ onCatalogChanged }: { onCatalogChanged?: () => void } = {}) {
  const { t } = useT();
  const [snapshot, setSnapshot] = useState<ModelCatalogSnapshot | null>(null);
  const [form, setForm] = useState<ManualForm | null>(null);
  const [editing, setEditing] = useState<CatalogModelEntry | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // KT-588 — 637 models across 10 targets were rendered as ten stacked lists.
  // Finding one meant scrolling past the other 636.
  const [query, setQuery] = useState('');
  const [targetFilter, setTargetFilter] = useState('');
  const [sort, setSort] = useState<{ column: SortColumn; direction: 'asc' | 'desc' }>(
    { column: 'model', direction: 'asc' },
  );
  const [paging, setPaging] = useState({ key: '', count: PAGE_SIZE });

  const load = async () => {
    const value = await modelCatalogApi.list();
    setSnapshot(value);
    onCatalogChanged?.();
    return value;
  };
  // KT-587 — awaited before anything is written, and dropped if the section
  // unmounted while the catalogue was in flight.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const value = await modelCatalogApi.list();
        if (!cancelled) setSnapshot(value);
      } catch (err) {
        if (!cancelled) setError(String(err));
      }
    })();
    return () => { cancelled = true; };
  }, []);

  const models = useMemo(
    () => snapshot?.targets.flatMap(target => target.models) ?? [],
    [snapshot],
  );

  // One row per model, carrying the target it belongs to: the table sorts and
  // filters on that, and the reader needs to see it on the row.
  const rows = useMemo<CatalogRow[]>(
    () => snapshot?.targets.flatMap(target => target.models.map(model => ({
      model,
      targetId: target.runtime_target_id,
      targetLabel: target.target_label ?? target.runtime_target_id,
    }))) ?? [],
    [snapshot],
  );

  const visibleRows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const filtered = rows.filter(row => {
      if (targetFilter && row.targetId !== targetFilter) return false;
      if (!needle) return true;
      // The exact id matters as much as the label: it is what gets pasted into
      // a tier, and it is often the only thing the reader remembers.
      return `${row.model.display_alias ?? row.model.display_name} ${row.model.model_id} ${row.targetLabel}`
        .toLowerCase()
        .includes(needle);
    });
    const key = (row: CatalogRow) => {
      switch (sort.column) {
        case 'target': return row.targetLabel;
        case 'provenance': return row.model.provenance;
        case 'availability': return row.model.availability;
        case 'cost': return row.model.cost_hint ?? '';
        default: return row.model.display_alias ?? row.model.display_name;
      }
    };
    const factor = sort.direction === 'asc' ? 1 : -1;
    return [...filtered].sort((left, right) => {
      const compared = key(left).localeCompare(key(right), undefined, { numeric: true });
      // Ties fall back to the model name, so a sort by target is alphabetical
      // inside each target rather than in insertion order.
      if (compared !== 0) return compared * factor;
      return (left.model.display_alias ?? left.model.display_name)
        .localeCompare(right.model.display_alias ?? right.model.display_name);
    });
  }, [rows, query, targetFilter, sort]);

  const pagingKey = `${query}\u0000${targetFilter}\u0000${sort.column}\u0000${sort.direction}`;
  const shownCount = paging.key === pagingKey ? paging.count : PAGE_SIZE;
  const shownRows = visibleRows.slice(0, shownCount);

  const toggleSort = (column: SortColumn) => {
    setSort(current => current.column === column
      ? { column, direction: current.direction === 'asc' ? 'desc' : 'asc' }
      : { column, direction: 'asc' });
  };

  const openCreate = () => {
    setEditing(null);
    setForm(blankForm(snapshot));
    setError(null);
  };
  const openEdit = (entry: CatalogModelEntry) => {
    setEditing(entry);
    setForm({
      runtimeTargetId: entry.runtime_target_id,
      agentType: entry.agent_type,
      modelId: entry.model_id,
      displayName: entry.display_alias ?? entry.display_name,
      capabilities: entry.capabilities,
      reasoningModes: entry.reasoning_modes.join(', '),
      tier: entry.tier_assignment ?? '',
      costHint: entry.cost_hint ?? '',
      privacyNote: entry.privacy_note ?? '',
    });
    setError(null);
  };
  const save = async () => {
    if (!form || !form.modelId.trim() || !form.displayName.trim()) return;
    setBusy(true);
    setError(null);
    const request = {
      runtime_target_id: form.runtimeTargetId,
      agent_type: form.agentType,
      model_id: form.modelId.trim(),
      display_name: form.displayName.trim(),
      capabilities: form.capabilities,
      reasoning_modes: form.reasoningModes.split(',').map(value => value.trim()).filter(Boolean),
      default_reasoning_mode: null,
      tier_assignment: form.tier || null,
      cost_hint: form.costHint || null,
      privacy_note: form.privacyNote.trim() || null,
    };
    try {
      if (editing) await modelCatalogApi.updateManual(request);
      else await modelCatalogApi.createManual(request);
      await load();
      setForm(null);
      setEditing(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };
  const remove = async (entry: CatalogModelEntry) => {
    setBusy(true);
    setError(null);
    try {
      await modelCatalogApi.deleteManual({
        runtime_target_id: entry.runtime_target_id,
        model_id: entry.model_id,
      });
      await load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="set-section set-model-catalog" data-testid="model-catalog-section">
      <div className="set-section-header-lg">
        <div>
          <h3>{t('modelCatalog.title')}</h3>
          <p className="set-hint">{t('modelCatalog.description')}</p>
        </div>
      </div>

      {error && <p className="set-hint" data-status="error">{error}</p>}

      {form && (
        <div className="set-ext-api-form set-model-catalog-form">
          <div className="set-ext-api-fields">
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.target')}</span>
              <select
                className="set-litellm-input"
                value={form.runtimeTargetId}
                disabled={Boolean(editing)}
                onChange={event => {
                  const target = snapshot?.targets.find(value => value.runtime_target_id === event.target.value);
                  if (target) setForm(current => current && ({
                    ...current,
                    runtimeTargetId: target.runtime_target_id,
                    agentType: target.agent_type,
                  }));
                }}
              >
                {snapshot?.targets.map(target => (
                  <option key={target.runtime_target_id} value={target.runtime_target_id}>
                    {target.target_label ?? target.runtime_target_id}
                  </option>
                ))}
              </select>
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.modelId')}</span>
              <input className="set-litellm-input" value={form.modelId} disabled={Boolean(editing)} onChange={event => setForm(current => current && ({ ...current, modelId: event.target.value }))} />
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.displayName')}</span>
              <input className="set-litellm-input" value={form.displayName} onChange={event => setForm(current => current && ({ ...current, displayName: event.target.value }))} />
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.tier')}</span>
              <select className="set-litellm-input" value={form.tier} onChange={event => setForm(current => current && ({ ...current, tier: event.target.value as ModelTier | '' }))}>
                <option value="">{t('modelCatalog.noTier')}</option>
                <option value="economy">⚡ {t('disc.tier.economy')}</option>
                <option value="default">🎯 {t('disc.tier.default')}</option>
                <option value="reasoning">🧠 {t('disc.tier.reasoning')}</option>
              </select>
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.reasoningModes')}</span>
              <input className="set-litellm-input" value={form.reasoningModes} placeholder="low, medium, high" onChange={event => setForm(current => current && ({ ...current, reasoningModes: event.target.value }))} />
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.costHintField')}</span>
              <select className="set-litellm-input" value={form.costHint} onChange={event => setForm(current => current && ({ ...current, costHint: event.target.value as ModelCostHint | '' }))}>
                <option value="">{t('modelCatalog.noCostHint')}</option>
                <option value="free">{t('modelCatalog.costHint.free')}</option>
                <option value="paid">{t('modelCatalog.costHint.paid')}</option>
                <option value="unknown">{t('modelCatalog.costHint.unknown')}</option>
              </select>
            </label>
            <label className="set-litellm-field">
              <span className="set-litellm-label">{t('modelCatalog.privacyNoteField')}</span>
              <input className="set-litellm-input" value={form.privacyNote} onChange={event => setForm(current => current && ({ ...current, privacyNote: event.target.value }))} />
            </label>
          </div>
          <div className="set-ext-api-test-actions">
            {(['chat', 'image', 'video'] as const).map(capability => (
              <label key={capability} className="set-model-catalog-capability">
                <input type="checkbox" checked={form.capabilities.includes(capability)} onChange={event => setForm(current => current && ({
                  ...current,
                  capabilities: event.target.checked
                    ? [...current.capabilities, capability]
                    : current.capabilities.filter(value => value !== capability),
                }))} />
                {capability}
              </label>
            ))}
            <button type="button" className="set-btn-primary" disabled={busy} onClick={() => void save()}>{t('common.save')}</button>
            <button type="button" className="set-icon-btn" onClick={() => setForm(null)} aria-label={t('common.cancel')}><X size={13} /></button>
          </div>
        </div>
      )}

      {/* The sources, as one line each rather than as ten containers: what a
          reader does here is re-check one of them, not browse it. */}
      <div className="set-model-catalog-sources">
        {snapshot?.targets.map(target => (
          <span
            key={target.runtime_target_id}
            className="set-model-catalog-source"
            data-stale={target.stale}
            data-selected={targetFilter === target.runtime_target_id}
          >
            <button
              type="button"
              className="set-model-catalog-source-name"
              onClick={() => setTargetFilter(
                targetFilter === target.runtime_target_id ? '' : target.runtime_target_id,
              )}
              title={target.stale ? t('modelCatalog.stale') : t('modelCatalog.current')}
            >
              {target.target_label ?? target.runtime_target_id}
              <em>{target.models.length}</em>
            </button>
            {!target.runtime_target_id.startsWith('http:') && (
              <button type="button" className="set-icon-btn" disabled={busy} aria-label={t('modelCatalog.recheck')} onClick={async () => {
                setBusy(true);
                try {
                  await modelCatalogApi.refresh({ runtime_target_id: target.runtime_target_id, agent_type: target.agent_type, force: true });
                  await load();
                } catch (err) { setError(String(err)); } finally { setBusy(false); }
              }}><RefreshCw size={11} /></button>
            )}
          </span>
        ))}
      </div>

      <div className="set-model-catalog-toolbar">
        <label className="set-model-catalog-search">
          <Search size={13} aria-hidden="true" />
          <input
            type="search"
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('modelCatalog.searchPlaceholder')}
            aria-label={t('modelCatalog.searchPlaceholder')}
            data-testid="model-catalog-search"
          />
        </label>
        {targetFilter && (
          <button
            type="button"
            className="set-model-catalog-clear-filter"
            onClick={() => setTargetFilter('')}
            data-testid="model-catalog-clear-filter"
          >
            <X size={11} /> {t('modelCatalog.allTargets')}
          </button>
        )}
      </div>

      {visibleRows.length === 0 ? (
        <p className="set-hint">{models.length === 0 ? t('modelCatalog.empty') : t('modelCatalog.noMatch')}</p>
      ) : (
        <div className="set-model-catalog-table-wrap">
          <table className="set-model-catalog-table" data-testid="model-catalog-table">
            <thead>
              <tr>
                {([
                  ['model', 'modelCatalog.columnModel'],
                  ['target', 'modelCatalog.columnTarget'],
                  ['provenance', 'modelCatalog.columnProvenance'],
                  ['cost', 'modelCatalog.columnCost'],
                ] as const).map(([column, label]) => (
                  <th key={column} aria-sort={sort.column === column
                    ? (sort.direction === 'asc' ? 'ascending' : 'descending')
                    : 'none'}>
                    <button
                      type="button"
                      onClick={() => toggleSort(column)}
                      data-testid={`model-catalog-sort-${column}`}
                    >
                      {t(label)}
                      {sort.column === column && (sort.direction === 'asc'
                        ? <ArrowUp size={10} aria-hidden="true" />
                        : <ArrowDown size={10} aria-hidden="true" />)}
                    </button>
                  </th>
                ))}
                <th aria-label={t('common.actions')} />
              </tr>
            </thead>
            <tbody>
              {shownRows.map(row => (
                <tr
                  key={row.model.id}
                  data-availability={row.model.availability}
                  data-testid={`model-catalog-row-${row.model.id}`}
                >
                  <td>
                    <button type="button" className="set-model-catalog-model-open" onClick={() => openEdit(row.model)}>
                      <span>{row.model.display_alias ?? row.model.display_name}</span>
                      <small>{row.model.model_id}</small>
                    </button>
                  </td>
                  <td><span className="set-model-catalog-target-cell">{row.targetLabel}</span></td>
                  <td>
                    {t(`modelCatalog.provenance.${row.model.provenance}`)}
                    {row.model.availability === 'unavailable' && (
                      <em className="set-model-catalog-unavailable">{t('modelCatalog.unavailable')}</em>
                    )}
                  </td>
                  <td>
                    {row.model.cost_hint && (
                      <span
                        className="badge badge-muted"
                        data-cost-hint={row.model.cost_hint}
                        title={row.model.privacy_note ?? undefined}
                      >
                        {t(`modelCatalog.costHint.${row.model.cost_hint}`)}
                      </span>
                    )}
                  </td>
                  <td>
                    {(row.model.provenance === 'manual' || row.model.provenance === 'migrated') && (
                      <button type="button" className="set-icon-btn" aria-label={t('common.delete')} onClick={() => void remove(row.model)}><Trash2 size={10} /></button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {shownRows.length < visibleRows.length && (
            <button
              type="button"
              className="set-model-catalog-more"
              onClick={() => setPaging({ key: pagingKey, count: shownCount + PAGE_SIZE })}
              data-testid="model-catalog-more"
            >
              {t('modelCatalog.showMore', visibleRows.length - shownRows.length)}
            </button>
          )}
        </div>
      )}
      <div className="set-model-catalog-footer">
        <button
          type="button"
          className="set-model-catalog-add"
          onClick={openCreate}
          title={t('modelCatalog.addHint')}
          data-testid="model-catalog-add"
        >
          <Plus size={11} /> {t('modelCatalog.add')}
        </button>
        {models.length > 0 && (
        <p className="set-hint">
          {visibleRows.length === models.length
            ? t('modelCatalog.count', models.length)
            : t('modelCatalog.shown', visibleRows.length, models.length)}
        </p>
        )}
      </div>
    </section>
  );
}
