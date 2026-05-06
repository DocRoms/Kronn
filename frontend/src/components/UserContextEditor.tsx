// UserContextEditor — manages files in ~/.kronn/user-context/.
//
// Cross-project, cross-CLI agent context : markdown files here are auto-
// injected into every agent's prompt. Without this UI, the operator had
// to open a terminal to manage them — for many users that's a wall.
//
// UX : a list of files (alphabetical), each row collapses an inline
// editor. Add a new file by typing a name + clicking Add. Delete via
// the trash icon. Saves are explicit (operator clicks "Enregistrer") so
// nothing changes mid-keystroke.

import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { userContext as api, type UserContextFile } from '../lib/api';
import { ChevronRight, Trash2, Plus, Loader2, AlertTriangle, Check } from 'lucide-react';

type RowState =
  | { kind: 'collapsed' }
  | { kind: 'loading' }
  | { kind: 'editing'; original: string; current: string }
  | { kind: 'saving'; current: string }
  | { kind: 'error'; current: string; msg: string };

export function UserContextEditor() {
  const { t } = useT();
  const [files, setFiles] = useState<UserContextFile[] | null>(null);
  const [rowStates, setRowStates] = useState<Record<string, RowState>>({});
  const [listError, setListError] = useState<string | null>(null);
  const [newName, setNewName] = useState('');
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const list = await api.list();
      setFiles(list);
      setListError(null);
    } catch (e) {
      setListError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => { refresh(); }, []);

  const setRow = (name: string, state: RowState) =>
    setRowStates(prev => ({ ...prev, [name]: state }));

  const onExpand = async (name: string) => {
    const cur = rowStates[name] ?? { kind: 'collapsed' };
    if (cur.kind !== 'collapsed') {
      setRow(name, { kind: 'collapsed' });
      return;
    }
    setRow(name, { kind: 'loading' });
    try {
      const file = await api.get(name);
      const content = file.content ?? '';
      setRow(name, { kind: 'editing', original: content, current: content });
    } catch (e) {
      setRow(name, { kind: 'error', current: '', msg: e instanceof Error ? e.message : String(e) });
    }
  };

  const onChange = (name: string, current: string) => {
    setRowStates(prev => {
      const cur = prev[name];
      if (!cur) return prev;
      if (cur.kind === 'editing') return { ...prev, [name]: { ...cur, current } };
      if (cur.kind === 'error') return { ...prev, [name]: { kind: 'editing', original: '', current } };
      return prev;
    });
  };

  const onSave = async (name: string) => {
    const cur = rowStates[name];
    if (!cur || (cur.kind !== 'editing' && cur.kind !== 'error')) return;
    const content = cur.current;
    setRow(name, { kind: 'saving', current: content });
    try {
      await api.put(name, content);
      setRow(name, { kind: 'editing', original: content, current: content });
      await refresh();
    } catch (e) {
      setRow(name, { kind: 'error', current: content, msg: e instanceof Error ? e.message : String(e) });
    }
  };

  const onDelete = async (name: string) => {
    if (!confirm(t('userContext.deleteConfirm', name))) return;
    try {
      await api.delete(name);
      setRowStates(prev => {
        const next = { ...prev };
        delete next[name];
        return next;
      });
      await refresh();
    } catch (e) {
      setListError(e instanceof Error ? e.message : String(e));
    }
  };

  const onCreate = async () => {
    let n = newName.trim();
    if (!n) return;
    if (!n.endsWith('.md')) n = `${n}.md`;
    setCreating(true);
    setCreateError(null);
    try {
      await api.put(n, `# ${n.replace(/\.md$/, '')}\n\n`);
      setNewName('');
      await refresh();
      setRow(n, { kind: 'editing', original: `# ${n.replace(/\.md$/, '')}\n\n`, current: `# ${n.replace(/\.md$/, '')}\n\n` });
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="user-context-editor">
      <div className="user-context-header">
        <h3 className="text-base font-semibold">{t('userContext.title')}</h3>
        <p className="text-xs text-muted">{t('userContext.subtitle')}</p>
      </div>

      <div className="user-context-create">
        <input
          type="text"
          className="user-context-name-input"
          placeholder={t('userContext.newNamePlaceholder')}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') onCreate(); }}
          disabled={creating}
          aria-label={t('userContext.newNameAria')}
        />
        <button
          className="user-context-add-btn"
          onClick={onCreate}
          disabled={creating || !newName.trim()}
        >
          {creating ? <Loader2 size={11} className="spin" /> : <Plus size={11} />}
          {t('userContext.add')}
        </button>
      </div>
      {createError && <div className="user-context-error"><AlertTriangle size={11} /> {createError}</div>}

      {listError && <div className="user-context-error"><AlertTriangle size={11} /> {listError}</div>}

      {files === null ? (
        <div className="user-context-loading"><Loader2 size={12} className="spin" /> {t('userContext.loading')}</div>
      ) : files.length === 0 ? (
        <div className="user-context-empty">{t('userContext.empty')}</div>
      ) : (
        <ul className="user-context-list" data-testid="user-context-list">
          {files.map((f) => {
            const state = rowStates[f.name] ?? { kind: 'collapsed' as const };
            const expanded = state.kind !== 'collapsed';
            return (
              <li key={f.name} className="user-context-row" data-name={f.name}>
                <div className="user-context-row-head">
                  <button
                    type="button"
                    className="user-context-toggle"
                    onClick={() => onExpand(f.name)}
                    aria-expanded={expanded}
                  >
                    <ChevronRight size={9} className="disc-chevron" data-expanded={expanded} />
                    <span className="user-context-name">{f.name}</span>
                    <span className="text-xs text-faint">{f.size} B</span>
                  </button>
                  <button
                    type="button"
                    className="user-context-delete"
                    onClick={() => onDelete(f.name)}
                    title={t('userContext.delete')}
                    aria-label={t('userContext.deleteAria', f.name)}
                  >
                    <Trash2 size={10} />
                  </button>
                </div>
                {state.kind === 'loading' && (
                  <div className="user-context-loading"><Loader2 size={11} className="spin" /> {t('userContext.loading')}</div>
                )}
                {(state.kind === 'editing' || state.kind === 'saving' || state.kind === 'error') && (
                  <div className="user-context-edit">
                    <textarea
                      className="user-context-textarea"
                      value={state.current}
                      onChange={(e) => onChange(f.name, e.target.value)}
                      disabled={state.kind === 'saving'}
                      rows={Math.max(6, Math.min(20, state.current.split('\n').length + 1))}
                      aria-label={t('userContext.editingAria', f.name)}
                    />
                    {state.kind === 'error' && (
                      <div className="user-context-error"><AlertTriangle size={11} /> {state.msg}</div>
                    )}
                    <div className="user-context-edit-actions">
                      <button
                        type="button"
                        className="user-context-save"
                        onClick={() => onSave(f.name)}
                        disabled={
                          state.kind === 'saving'
                          || (state.kind === 'editing' && state.current === state.original)
                        }
                      >
                        {state.kind === 'saving'
                          ? <Loader2 size={11} className="spin" />
                          : <Check size={11} />}
                        {t('userContext.save')}
                      </button>
                    </div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
