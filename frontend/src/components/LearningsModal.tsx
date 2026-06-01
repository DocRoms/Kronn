// 0.9.0 — Continual Learning validation modal. THE human gate (posture B): in
// the default config nothing is written to a truth file until the user clicks
// Validate here. Lists pending learnings with their type, confidence, evidence,
// and the Gate-2 faithfulness verdict (informative — a 🔴 contradiction is a
// "double-check", not an auto-block).

import { Check, X, BookOpen, AlertTriangle } from 'lucide-react';
import type { Learning, LearningKind, Faithfulness } from '../types/generated';
import './LearningsModal.css';

export interface LearningsModalProps {
  learnings: Learning[];
  onValidate: (id: string) => void;
  onReject: (id: string) => void;
  onClose: () => void;
  busyId?: string | null;
  t: (key: string, ...args: (string | number)[]) => string;
}

const KIND_KEY: Record<LearningKind, string> = {
  fact: 'disc.learningKindFact',
  preference: 'disc.learningKindPreference',
  inference: 'disc.learningKindInference',
};

const FAITH_KEY: Record<Faithfulness, string> = {
  entailment: 'disc.learningFaithEntailment',
  neutral: 'disc.learningFaithNeutral',
  contradiction: 'disc.learningFaithContradiction',
};

export function LearningsModal({
  learnings,
  onValidate,
  onReject,
  onClose,
  busyId,
  t,
}: LearningsModalProps) {
  return (
    <div className="learnings-modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="learnings-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="learnings-modal-title"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="learnings-modal-header">
          <div className="learnings-modal-icon">
            <BookOpen size={18} />
          </div>
          <h3 id="learnings-modal-title">{t('disc.learningsTitle')}</h3>
          <button
            className="learnings-modal-close"
            onClick={onClose}
            title={t('disc.learningClose')}
            aria-label={t('disc.learningClose')}
          >
            <X size={14} />
          </button>
        </header>

        <div className="learnings-modal-body">
          {learnings.length === 0 ? (
            <p className="learnings-modal-empty">{t('disc.learningsEmpty')}</p>
          ) : (
            <ul className="learnings-list">
              {learnings.map((l) => {
                const busy = busyId === l.id;
                return (
                  <li key={l.id} className="learning-card" data-kind={l.kind}>
                    <div className="learning-card-head">
                      <span className="learning-kind-chip" data-kind={l.kind}>
                        {t(KIND_KEY[l.kind])}
                      </span>
                      {typeof l.confidence === 'number' && (
                        <span className="learning-confidence" title={t('disc.learningConfidence')}>
                          {Math.round(l.confidence * 100)}%
                        </span>
                      )}
                      {l.faithfulness && (
                        <span className="learning-faith-chip" data-verdict={l.faithfulness}>
                          {l.faithfulness === 'contradiction' && <AlertTriangle size={11} />}
                          {t(FAITH_KEY[l.faithfulness])}
                        </span>
                      )}
                    </div>

                    <p className="learning-claim">{l.claim}</p>

                    {l.evidence.length > 0 && (
                      <ul className="learning-evidence">
                        {l.evidence.map((e, i) => (
                          <li key={i} className="learning-evidence-row" data-kind={e.kind}>
                            <span className="learning-evidence-kind">{e.kind}</span>
                            <code className="learning-evidence-ref">{e.ref}</code>
                            {e.quote && <span className="learning-evidence-quote">{e.quote}</span>}
                          </li>
                        ))}
                      </ul>
                    )}

                    <div className="learning-card-actions">
                      <button
                        className="learning-btn learning-btn-validate"
                        disabled={busy}
                        onClick={() => onValidate(l.id)}
                      >
                        <Check size={13} /> {t('disc.learningValidate')}
                      </button>
                      <button
                        className="learning-btn learning-btn-reject"
                        disabled={busy}
                        onClick={() => onReject(l.id)}
                      >
                        <X size={13} /> {t('disc.learningReject')}
                      </button>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
