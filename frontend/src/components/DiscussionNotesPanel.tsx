// KT-580 — the notes of a discussion, in one place.
//
// The inversion this fixes: `disc_note_list` gave AGENTS a clean list of a
// room's notes, served by a route this side never called. A human wrote a note
// for themselves, and only the agent could read it back properly — theirs were
// mixed into the transcript, hidden behind a global switch that showed all or
// none, with no way to list, correct or remove one.
import { useCallback, useEffect, useState } from 'react';
import { Loader2, NotebookPen, Pencil, Plus, Trash2, X, Check } from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import type { DiscussionNote } from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import './DiscussionNotesPanel.css';

export interface DiscussionNotesPanelProps {
  discussionId: string;
  /** Bumped by the page when a note is written from the composer, so the list
   *  does not wait for a reopen to show it. */
  refreshKey?: number;
  onClose: () => void;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function DiscussionNotesPanel({
  discussionId,
  refreshKey = 0,
  onClose,
  toast,
  t,
}: DiscussionNotesPanelProps) {
  const [loaded, setLoaded] = useState<{
    discussionId: string; notes: DiscussionNote[]; nextCursor: number | null; total: number;
  } | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<{ id: string; content: string } | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  // Only the answer that belongs to the room on screen. Switching discussions
  // must not show the previous room's notes while the new ones load.
  const notes = loaded?.discussionId === discussionId ? loaded.notes : null;

  const load = useCallback(async () => {
    try {
      const response = await discussionsApi.notes(discussionId);
      setLoaded({
        discussionId,
        notes: response.notes,
        nextCursor: response.next_cursor,
        total: response.total_notes,
      });
      setError(null);
    } catch (loadError) {
      setError(String(loadError));
    }
  }, [discussionId]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const response = await discussionsApi.notes(discussionId);
        if (!cancelled) {
          setLoaded({
            discussionId,
            notes: response.notes,
            nextCursor: response.next_cursor,
            total: response.total_notes,
          });
          setError(null);
        }
      } catch (loadError) {
        if (!cancelled) setError(String(loadError));
      }
    })();
    return () => { cancelled = true; };
  }, [discussionId, refreshKey]);

  const loadMore = async () => {
    if (!loaded || loaded.nextCursor == null || loadingMore) return;
    setLoadingMore(true);
    try {
      const response = await discussionsApi.notes(discussionId, loaded.nextCursor);
      setLoaded(current => (current && current.discussionId === discussionId
        ? {
            ...current,
            notes: [...current.notes, ...response.notes],
            nextCursor: response.next_cursor,
            total: response.total_notes,
          }
        : current));
    } catch (moreError) {
      toast(String(moreError), 'error');
    } finally {
      setLoadingMore(false);
    }
  };

  const addNote = async () => {
    const content = draft.trim();
    if (!content || adding) return;
    setAdding(true);
    try {
      await discussionsApi.sendNote(discussionId, content);
      setDraft('');
      await load();
    } catch (sendError) {
      toast(String(sendError), 'error');
    } finally {
      setAdding(false);
    }
  };

  const saveEdit = async () => {
    if (!editing) return;
    const content = editing.content.trim();
    if (!content) return;
    setBusyId(editing.id);
    try {
      await discussionsApi.reviseNote(discussionId, editing.id, content);
      setEditing(null);
      await load();
    } catch (saveError) {
      toast(String(saveError), 'error');
    } finally {
      setBusyId(null);
    }
  };

  const removeNote = async (note: DiscussionNote) => {
    if (!confirm(t('disc.note.deleteConfirm'))) return;
    setBusyId(note.message.id);
    try {
      await discussionsApi.deleteMessage(discussionId, note.message.id);
      await load();
    } catch (deleteError) {
      toast(String(deleteError), 'error');
    } finally {
      setBusyId(null);
    }
  };

  return (
    <section className="disc-notes-panel" data-testid="disc-notes-panel">
      <header className="disc-notes-panel-head">
        <span className="disc-notes-panel-title">
          <NotebookPen size={14} aria-hidden="true" />
          {t('disc.note.panelTitle')}
          {notes && <em>{loaded?.total ?? notes.length}</em>}
        </span>
        <button type="button" className="set-icon-btn" onClick={onClose} aria-label={t('common.close')}>
          <X size={13} />
        </button>
      </header>

      {/* Writing and reading in the same place: the old gesture asked the
          author to understand a channel toggle in the composer first. */}
      <div className="disc-notes-compose">
        <textarea
          value={draft}
          onChange={event => setDraft(event.target.value)}
          placeholder={t('disc.note.addPlaceholder')}
          aria-label={t('disc.note.addPlaceholder')}
          data-testid="disc-note-draft"
          rows={2}
        />
        <button
          type="button"
          className="disc-notes-add"
          onClick={() => void addNote()}
          disabled={!draft.trim() || adding}
          data-testid="disc-note-add"
        >
          {adding ? <Loader2 size={12} className="spin" /> : <Plus size={12} />}
          {t('disc.note.add')}
        </button>
      </div>

      {error && <p className="disc-notes-error" data-testid="disc-notes-error">{error}</p>}

      {notes === null && !error && (
        <p className="disc-notes-loading"><Loader2 size={13} className="spin" /></p>
      )}

      {notes?.length === 0 && (
        <p className="disc-notes-empty" data-testid="disc-notes-empty">{t('disc.note.empty')}</p>
      )}

      <ul className="disc-notes-list">
        {notes?.map(note => {
          const isEditing = editing?.id === note.message.id;
          const busy = busyId === note.message.id;
          return (
            <li key={note.message.id} className="disc-notes-item" data-testid={`disc-note-${note.message.id}`}>
              {isEditing ? (
                <>
                  <textarea
                    value={editing.content}
                    onChange={event => setEditing({ id: editing.id, content: event.target.value })}
                    aria-label={t('disc.note.editing')}
                    data-testid="disc-note-edit-field"
                    rows={3}
                    autoFocus
                  />
                  <div className="disc-notes-item-actions">
                    <button
                      type="button"
                      className="set-icon-btn"
                      onClick={() => void saveEdit()}
                      disabled={!editing.content.trim() || busy}
                      aria-label={t('common.save')}
                      data-testid="disc-note-save"
                    >
                      {busy ? <Loader2 size={12} className="spin" /> : <Check size={12} />}
                    </button>
                    <button
                      type="button"
                      className="set-icon-btn"
                      onClick={() => setEditing(null)}
                      aria-label={t('common.cancel')}
                    >
                      <X size={12} />
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <p className="disc-notes-item-content">{note.message.content}</p>
                  <div className="disc-notes-item-meta">
                    <time dateTime={note.message.timestamp}>
                      {new Date(note.message.timestamp).toLocaleString()}
                    </time>
                    <span className="disc-notes-item-actions">
                      <button
                        type="button"
                        className="set-icon-btn"
                        onClick={() => setEditing({ id: note.message.id, content: note.message.content })}
                        aria-label={t('disc.note.edit')}
                        data-testid={`disc-note-edit-${note.message.id}`}
                      >
                        <Pencil size={11} />
                      </button>
                      <button
                        type="button"
                        className="set-icon-btn text-ghost"
                        onClick={() => void removeNote(note)}
                        disabled={busy}
                        aria-label={t('common.delete')}
                        data-testid={`disc-note-delete-${note.message.id}`}
                      >
                        {busy ? <Loader2 size={11} className="spin" /> : <Trash2 size={11} />}
                      </button>
                    </span>
                  </div>
                </>
              )}
            </li>
          );
        })}
      </ul>

      {loaded?.discussionId === discussionId && loaded.nextCursor != null && (
        <button
          type="button"
          className="disc-notes-more"
          onClick={() => void loadMore()}
          disabled={loadingMore}
          data-testid="disc-notes-more"
        >
          {loadingMore ? <Loader2 size={12} className="spin" /> : null}
          {t('disc.note.showMore', loaded.total - loaded.notes.length)}
        </button>
      )}
    </section>
  );
}
