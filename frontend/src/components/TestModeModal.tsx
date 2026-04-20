// Preflight modal — shown when `test-mode/enter` refuses because the
// repos aren't in a safe state. Gives the user a clear, explicit choice
// per blocker kind instead of a free-form error toast.

import { AlertTriangle, FlaskConical, FileEdit, Check, X, Archive } from 'lucide-react';
import type { TestModeBlocker } from '../types/extensions';

export interface TestModeModalProps {
  /// The blocker returned by the last `testModeEnter` attempt — drives the
  /// modal content via `.kind` (WorktreeDirty | MainDirty | Detached).
  blocker: TestModeBlocker;
  /// Retry the enter call with extra options once the user picks a path
  /// (e.g. `stash_dirty: true` on MainDirty). Parent handles the API call.
  onRetry: (opts: { stash_dirty?: boolean; force?: boolean }) => void;
  /// "Commit first" — dismiss the modal and open the GitPanel so the user
  /// can commit the worktree or main-repo changes before retrying. Parent
  /// wires this to `setShowGitPanel(true)`.
  onGoCommit: () => void;
  onCancel: () => void;
  busy: boolean;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function TestModeModal({ blocker, onRetry, onGoCommit, onCancel, busy, t }: TestModeModalProps) {
  const files = blocker.details?.files ?? [];
  const currentBranch = blocker.details?.current_branch ?? '';

  return (
    <div className="test-mode-modal-backdrop" role="presentation" onClick={onCancel}>
      <div
        className="test-mode-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="test-mode-modal-title"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="test-mode-modal-header">
          <div className="test-mode-modal-icon">
            {blocker.kind === 'WorktreeDirty' ? <FileEdit size={18} /> : <AlertTriangle size={18} />}
          </div>
          <h3 id="test-mode-modal-title">
            {blocker.kind === 'WorktreeDirty' && t('testMode.modal.worktreeDirtyTitle')}
            {blocker.kind === 'MainDirty' && t('testMode.modal.mainDirtyTitle', currentBranch || '—')}
            {blocker.kind === 'Detached' && t('testMode.modal.detachedTitle')}
            {blocker.kind !== 'WorktreeDirty' && blocker.kind !== 'MainDirty' && blocker.kind !== 'Detached' && t('testMode.modal.fallbackTitle')}
          </h3>
          <button className="test-mode-modal-close" onClick={onCancel} title={t('testMode.modal.cancel')}>
            <X size={14} />
          </button>
        </header>

        <div className="test-mode-modal-body">
          <p className="test-mode-modal-explain">
            {blocker.kind === 'WorktreeDirty' && t('testMode.modal.worktreeDirtyExplain')}
            {blocker.kind === 'MainDirty' && t('testMode.modal.mainDirtyExplain')}
            {blocker.kind === 'Detached' && t('testMode.modal.detachedExplain')}
            {blocker.kind !== 'WorktreeDirty' && blocker.kind !== 'MainDirty' && blocker.kind !== 'Detached' && blocker.message}
          </p>

          {files.length > 0 && (
            <ul className="test-mode-modal-files">
              {/* Caps the visible list to keep the modal compact; full list
                  is always available via the GitPanel. */}
              {files.slice(0, 8).map((f, i) => (
                <li key={i}>
                  <code className="test-mode-modal-file-status">{f.status}</code>
                  <span className="test-mode-modal-file-path">{f.path}</span>
                </li>
              ))}
              {files.length > 8 && (
                <li className="test-mode-modal-files-more">
                  {t('testMode.modal.filesMore', files.length - 8)}
                </li>
              )}
            </ul>
          )}
        </div>

        <footer className="test-mode-modal-actions">
          {/* Action matrix varies per kind — see skill `plan` for rationale:
              - WorktreeDirty: the agent owns the worktree → only safe action
                is "go commit it" (in GitPanel), which the user can do in a
                single click. Retrying with any flag would lose work.
              - MainDirty: 3 options — stash auto (opt-in), commit first,
                or cancel. We default the stash button to 'primary' so it's
                the visible first-choice action.
              - Detached: 2 options — proceed anyway (force=true) or cancel.
          */}
          {blocker.kind === 'WorktreeDirty' && (
            <>
              <button className="test-mode-modal-btn primary" onClick={onGoCommit} disabled={busy}>
                <FlaskConical size={12} />
                {t('testMode.modal.openGitPanel')}
              </button>
              <button className="test-mode-modal-btn ghost" onClick={onCancel} disabled={busy}>
                {t('testMode.modal.cancel')}
              </button>
            </>
          )}
          {blocker.kind === 'MainDirty' && (
            <>
              <button
                className="test-mode-modal-btn primary"
                onClick={() => onRetry({ stash_dirty: true })}
                disabled={busy}
              >
                <Archive size={12} />
                {busy ? t('testMode.modal.stashing') : t('testMode.modal.stashAndProceed')}
              </button>
              <button className="test-mode-modal-btn ghost" onClick={onGoCommit} disabled={busy}>
                <FlaskConical size={12} />
                {t('testMode.modal.commitFirst')}
              </button>
              <button className="test-mode-modal-btn ghost" onClick={onCancel} disabled={busy}>
                {t('testMode.modal.cancel')}
              </button>
            </>
          )}
          {blocker.kind === 'Detached' && (
            <>
              <button
                className="test-mode-modal-btn primary"
                onClick={() => onRetry({ force: true })}
                disabled={busy}
              >
                <Check size={12} />
                {t('testMode.modal.proceedAnyway')}
              </button>
              <button className="test-mode-modal-btn ghost" onClick={onCancel} disabled={busy}>
                {t('testMode.modal.cancel')}
              </button>
            </>
          )}
          {blocker.kind !== 'WorktreeDirty' && blocker.kind !== 'MainDirty' && blocker.kind !== 'Detached' && (
            <button className="test-mode-modal-btn ghost" onClick={onCancel} disabled={busy}>
              {t('testMode.modal.cancel')}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
