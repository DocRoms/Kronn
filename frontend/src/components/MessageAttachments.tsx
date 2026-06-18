// 0.8.8 — render the files a user pinned to a message (Option B per-message
// attachments). Images become thumbnails fetched as auth'd blobs (an `<img
// src>` can't carry the auth header, so we fetch → object URL → revoke on
// unmount). Non-image files (no disk_path on the backend) render as a filename
// chip. Lives in its own file so the blob lifecycle is unit-testable in
// isolation from the heavy MessageBubble.
import { useEffect, useState } from 'react';
import { Image as ImageIcon, FileText } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions as discussionsApi } from '../lib/api';

type T = (key: string, ...args: (string | number)[]) => string;

function formatKb(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function AttachmentThumb({ file, discussionId, t }: { file: ContextFile; discussionId: string; t: T }) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  // The backend only stores a disk_path for images; text files extract to the
  // DB and have none. So disk_path is our "this is a renderable image" signal.
  const isImage = !!file.disk_path;

  useEffect(() => {
    if (!isImage) return;
    let cancelled = false;
    let objectUrl: string | null = null;
    discussionsApi.contextFileBlob(discussionId, file.id)
      .then((blob: Blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setUrl(objectUrl);
      })
      .catch(() => { if (!cancelled) setFailed(true); });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [isImage, discussionId, file.id]);

  const meta = `${file.filename} (${formatKb(file.original_size)})`;

  if (isImage && !failed) {
    return (
      <a
        className="disc-attach-thumb"
        href={url ?? undefined}
        target="_blank"
        rel="noreferrer"
        title={meta}
        aria-label={t('disc.attachmentImage', file.filename)}
      >
        {url
          ? <img src={url} alt={file.filename} loading="lazy" />
          : <span className="disc-attach-thumb-loading" aria-hidden="true"><ImageIcon size={14} /></span>}
      </a>
    );
  }

  // Non-image, or an image whose bytes failed to load → filename chip.
  return (
    <span className="disc-attach-chip" title={meta} data-testid="attach-chip">
      <FileText size={11} />
      <span className="disc-attach-chip-name">{file.filename}</span>
    </span>
  );
}

export function MessageAttachments({ files, discussionId, t }: { files: ContextFile[]; discussionId: string; t: T }) {
  if (!files.length) return null;
  return (
    <div className="disc-msg-attachments" data-testid="msg-attachments">
      {files.map(f => <AttachmentThumb key={f.id} file={f} discussionId={discussionId} t={t} />)}
    </div>
  );
}
