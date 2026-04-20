// Global "test mode" banner — stays pinned at the top of the discussions
// page whenever ANY discussion has its branch checked out in the main repo,
// regardless of which discussion is currently active. The banner is the
// single exit path (clicking "Stop" triggers test-mode/exit, which pops the
// stash and re-creates the worktree). See `backend/src/api/disc_git.rs`.

import { FlaskConical, GitBranch, X } from 'lucide-react';
import type { Discussion } from '../types/generated';

export interface TestModeBannerProps {
  /// The discussion currently in test mode. Caller is responsible for
  /// finding it via `discussions.find(d => d.test_mode_restore_branch)`.
  discussion: Discussion;
  /// User-invoked stop — parent calls `testModeExit`, clears DB fields, and
  /// refetches discussions so the banner disappears.
  onExit: () => void;
  /// True while the exit request is in-flight. Disables the button + shows
  /// the busy state; prevents double-clicks which would race with the
  /// rollback logic on the server.
  busy: boolean;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function TestModeBanner({ discussion, onExit, busy, t }: TestModeBannerProps) {
  const branch = discussion.worktree_branch ?? '';
  const restore = discussion.test_mode_restore_branch ?? '';
  return (
    <div className="test-mode-banner" role="status" aria-live="polite">
      <div className="test-mode-banner-icon">
        <FlaskConical size={16} />
      </div>
      <div className="test-mode-banner-body">
        <div className="test-mode-banner-headline">
          {t('testMode.bannerHeadline')}
        </div>
        <div className="test-mode-banner-subline">
          {/* Dev-friendly layer: exposes branch + restore so a dev reading the
              banner understands what's actually happening at the git level. */}
          <GitBranch size={10} />
          <code>{branch}</code>
          <span className="test-mode-banner-sep">·</span>
          <span>{t('testMode.bannerRestore', restore || '—')}</span>
          <span className="test-mode-banner-sep">·</span>
          <span className="test-mode-banner-disc">{discussion.title}</span>
        </div>
      </div>
      <button
        className="test-mode-banner-exit"
        onClick={onExit}
        disabled={busy}
        title={t('testMode.exitTooltip')}
      >
        <X size={12} />
        <span>{busy ? t('testMode.exiting') : t('testMode.exit')}</span>
      </button>
    </div>
  );
}
