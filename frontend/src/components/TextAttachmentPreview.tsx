import { useEffect, useRef, useState } from 'react';
import { Download, Loader2 } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions } from '../lib/api';
import { triggerDownload } from '../lib/downloadBlob';
import type { TextAttachmentPreview as Preview } from '../lib/textAttachmentPreview';

/** Mounted with a discussion/file key: pending replies belong to one asset. */
export function TextAttachmentPreview({ file, t }: {
  file: ContextFile;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [failed, setFailed] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [downloadFailed, setDownloadFailed] = useState(false);
  const downloadInFlight = useRef(false);

  useEffect(() => {
    const controller = new AbortController();
    discussions.contextFileTextPreview(file.discussion_id, file.id, controller.signal)
      .then(result => { if (!controller.signal.aborted) setPreview(result); })
      .catch(() => { if (!controller.signal.aborted) setFailed(true); });
    return () => controller.abort();
  }, [file.discussion_id, file.id]);

  const download = async () => {
    if (downloadInFlight.current) return;
    downloadInFlight.current = true;
    setDownloading(true);
    setDownloadFailed(false);
    try {
      const blob = await discussions.contextFileBlob(file.discussion_id, file.id);
      triggerDownload(file.filename, blob);
    } catch {
      setDownloadFailed(true);
    } finally {
      downloadInFlight.current = false;
      setDownloading(false);
    }
  };

  return (
    <section className="disc-text-preview" aria-label={t('disc.attachmentText', file.filename)}>
      <div className="disc-text-preview-toolbar">
        {preview?.truncated && <span role="note">{t('disc.attachmentTextTruncated')}</span>}
        <button type="button" className="disc-text-preview-download" onClick={() => void download()}
          disabled={downloading} aria-label={t('disc.assets.downloadFor', file.filename)}>
          {downloading ? <Loader2 size={15} className="spin" /> : <Download size={15} />}
          {t('disc.assets.download')}
        </button>
      </div>
      {downloadFailed && <p role="alert">{t('disc.assets.downloadFailed')}</p>}
      {failed ? <p role="alert">{t('disc.attachmentTextFailed')}</p>
        : preview ? preview.text.length
          ? <pre className="disc-text-preview-content" tabIndex={0}>{preview.text}</pre>
          : <p>{t('disc.attachmentTextEmpty')}</p>
        : <p role="status"><Loader2 size={20} className="spin" /> {t('disc.attachmentTextLoading')}</p>}
    </section>
  );
}
