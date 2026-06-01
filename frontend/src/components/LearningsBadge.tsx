// 0.9.0 — Global pending-learnings badge + modal trigger. Self-contained so the
// host (ChatHeader) only renders <LearningsBadge t toast />. Polls the pending
// count; on click opens the LearningsModal with the full pending list and wires
// Validate/Reject → api.learnings + local refresh. Hidden when count is 0 (so it
// stays invisible while the feature is OFF / nothing pending).

import { useCallback, useEffect, useRef, useState } from 'react';
import { BookOpen } from 'lucide-react';
import { learnings as learningsApi } from '../lib/api';
import type { Learning } from '../types/generated';
import { LearningsModal } from './LearningsModal';
import './LearningsBadge.css';

interface LearningsBadgeProps {
  t: (key: string, ...args: (string | number)[]) => string;
  toast: (msg: string, kind?: 'success' | 'error' | 'info') => void;
  /** Poll interval ms (0 = no polling, refresh on open/action only). */
  pollMs?: number;
}

export function LearningsBadge({ t, toast, pollMs = 60000 }: LearningsBadgeProps) {
  const [count, setCount] = useState(0);
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<Learning[]>([]);
  const [busyId, setBusyId] = useState<string | null>(null);
  const busyRef = useRef(false);

  const refreshCount = useCallback(() => {
    learningsApi
      .pending()
      .then((r) => setCount(r.count))
      .catch(() => {});
  }, []);

  useEffect(() => {
    refreshCount();
    if (!pollMs) return;
    const id = setInterval(refreshCount, pollMs);
    return () => clearInterval(id);
  }, [refreshCount, pollMs]);

  const openModal = useCallback(() => {
    learningsApi
      .list('pending')
      .then((rows) => {
        setItems(rows);
        setOpen(true);
      })
      .catch(() => toast(t('disc.learningsLoadError'), 'error'));
  }, [t, toast]);

  const resolve = useCallback(
    async (id: string, action: 'validate' | 'reject') => {
      if (busyRef.current) return;
      busyRef.current = true;
      setBusyId(id);
      try {
        if (action === 'validate') await learningsApi.validate(id);
        else await learningsApi.reject(id);
        setItems((prev) => prev.filter((l) => l.id !== id));
        setCount((c) => Math.max(0, c - 1));
        toast(
          t(action === 'validate' ? 'disc.learningValidated' : 'disc.learningRejected'),
          'success',
        );
      } catch {
        toast(t('disc.learningActionError'), 'error');
      } finally {
        busyRef.current = false;
        setBusyId(null);
      }
    },
    [t, toast],
  );

  if (count === 0 && !open) return null;

  return (
    <>
      <button
        type="button"
        className="learnings-badge"
        onClick={openModal}
        title={t('disc.learningsBadgeTitle')}
        aria-label={t('disc.learningsBadgeTitle')}
      >
        <BookOpen size={14} />
        <span className="learnings-badge-count">{count}</span>
      </button>
      {open && (
        <LearningsModal
          learnings={items}
          busyId={busyId}
          onValidate={(id) => resolve(id, 'validate')}
          onReject={(id) => resolve(id, 'reject')}
          onClose={() => {
            setOpen(false);
            refreshCount();
          }}
          t={t}
        />
      )}
    </>
  );
}
