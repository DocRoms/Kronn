import { useId, useState } from 'react';
import { Code2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { MarkdownContent } from './MessageBubble';
import {
  readTaskDescriptionMode,
  writeTaskDescriptionMode,
  type TaskDescriptionMode,
} from '../lib/taskDescriptionMode';
import './TaskDescription.css';

interface TaskDescriptionProps {
  value: string;
  /** Present when the description is editable; the raw view is then a textarea. */
  onChange?: (value: string) => void;
  className?: string;
}

/**
 * A task description, rendered as Markdown by default with a switch to the
 * raw source. The value stays owned by the caller, so switching never drops
 * an unsaved edit.
 */
export function TaskDescription({ value, onChange, className = '' }: TaskDescriptionProps) {
  const { t } = useT();
  const labelId = useId();
  const [mode, setModeState] = useState<TaskDescriptionMode>(readTaskDescriptionMode);
  const editable = onChange !== undefined;
  const raw = mode === 'raw';

  const setMode = (next: TaskDescriptionMode) => {
    setModeState(next);
    writeTaskDescriptionMode(next);
  };

  return (
    <div className={`task-description${className ? ` ${className}` : ''}`} data-mode={mode}>
      <div className="task-description-head">
        <span id={labelId} className="task-description-label">{t('planning.description')}</span>
        <button
          type="button"
          role="switch"
          aria-checked={raw}
          className="task-description-switch"
          aria-label={t(editable ? 'planning.descriptionRawHint' : 'planning.descriptionRawHintReadOnly')}
          title={t(editable ? 'planning.descriptionRawHint' : 'planning.descriptionRawHintReadOnly')}
          onClick={() => setMode(raw ? 'rendered' : 'raw')}
        >
          <Code2 size={12} aria-hidden="true" />
          <span>{t('planning.descriptionRaw')}</span>
          <span className="task-description-switch-track" aria-hidden="true">
            <span className="task-description-switch-thumb" />
          </span>
        </button>
      </div>
      {raw ? (
        editable ? (
          <textarea
            rows={7}
            aria-labelledby={labelId}
            value={value}
            onChange={event => onChange(event.target.value)}
          />
        ) : (
          <pre className="task-description-source" aria-labelledby={labelId}>{value}</pre>
        )
      ) : value.trim() ? (
        <div className="task-description-rendered" aria-labelledby={labelId} role="region">
          <MarkdownContent content={value} />
        </div>
      ) : (
        <div className="task-description-empty">
          <span>{t('planning.descriptionEmpty')}</span>
          {editable && (
            <button type="button" onClick={() => setMode('raw')}>
              {t('planning.descriptionWrite')}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
