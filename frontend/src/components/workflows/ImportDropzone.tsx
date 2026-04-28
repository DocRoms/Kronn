// 0.7.0 UX pass — generic file-import dropzone for Workflows / QPs.
// Combines a drag-and-drop area with a fallback file picker button.
// Reused on WorkflowsPage and QuickPromptsPage so the import UX is
// uniform.
//
// Design discipline:
//   - The component owns NO API logic — it just hands a parsed JSON
//     `content` string + optional preview to the caller via onFile.
//   - JSON validation is opportunistic here (we let the backend do the
//     authoritative check). Front-side we just confirm it parses + has
//     the right `kind` discriminator for the caller's expected type.
//   - Drag-over visual feedback is essential : a dropzone that doesn't
//     react when you hover a file feels broken (Marie noticed it on
//     similar UIs).

import { useState, useRef } from 'react';
import { Upload, FileJson, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';

export interface ImportDropzoneProps {
  /** Discriminator the file must contain (`"kronn.workflow"` or
   *  `"kronn.quick_prompt"`). Empty = accept any kind (debug only). */
  expectedKind: string;
  /** Called once the file is parsed + kind-validated. Caller decides
   *  whether to show a preview drawer, ask for project_id, or POST
   *  directly. The full parsed envelope is passed back so the caller
   *  can extract whatever it needs (e.g. preview steps, qp count). */
  onFile: (content: string, parsed: { kind: string; [k: string]: unknown }) => void;
  /** When set, the dropzone is rendered as part of a host drawer that
   *  has its own dismiss button — this hides our internal X. */
  embedded?: boolean;
  /** Called when the user clicks the X in standalone mode. */
  onCancel?: () => void;
}

export function ImportDropzone({
  expectedKind,
  onFile,
  embedded = false,
  onCancel,
}: ImportDropzoneProps) {
  const { t } = useT();
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const handleFile = async (file: File) => {
    setError(null);
    if (!file.name.endsWith('.json')) {
      setError(t('imp.errorNotJson'));
      return;
    }
    let text: string;
    try {
      text = await file.text();
    } catch {
      setError(t('imp.errorReadFailed'));
      return;
    }
    let parsed: { kind?: string; [k: string]: unknown };
    try {
      parsed = JSON.parse(text);
    } catch {
      setError(t('imp.errorJsonParse'));
      return;
    }
    if (expectedKind && parsed.kind !== expectedKind) {
      setError(t('imp.errorWrongKind').replace('{expected}', expectedKind).replace('{got}', String(parsed.kind ?? '?')));
      return;
    }
    onFile(text, parsed as { kind: string; [k: string]: unknown });
  };

  const onPickFile = () => fileInputRef.current?.click();

  return (
    <div
      className="wf-import-dropzone"
      data-drag-over={dragOver}
      onDragEnter={(e) => { e.preventDefault(); setDragOver(true); }}
      onDragLeave={(e) => { e.preventDefault(); setDragOver(false); }}
      onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        const file = e.dataTransfer.files?.[0];
        if (file) void handleFile(file);
      }}
    >
      {!embedded && onCancel && (
        <button
          className="wf-import-dropzone-close"
          onClick={onCancel}
          aria-label={t('imp.cancel')}
        ><X size={12} /></button>
      )}
      <Upload size={28} className="wf-import-dropzone-icon" />
      <div className="wf-import-dropzone-title">{t('imp.dropOrPick')}</div>
      <div className="wf-import-dropzone-hint">
        <FileJson size={11} />
        <span>{t('imp.fileHint')}</span>
      </div>
      <button className="wf-import-dropzone-pick-btn" onClick={onPickFile}>
        {t('imp.pickFile')}
      </button>
      <input
        ref={fileInputRef}
        type="file"
        accept=".json,application/json"
        style={{ display: 'none' }}
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) void handleFile(file);
          // Reset so picking the same file again retriggers the change event.
          e.target.value = '';
        }}
      />
      {error && <div className="wf-import-dropzone-error">{error}</div>}
    </div>
  );
}
