/** Product limit for putting a whole text file in the clipboard. */
export const COPY_TEXT_MAX_BYTES = 2 * 1024 * 1024;

export type CopyKind = 'text' | 'image' | 'video';

export type CopyRefusal = 'insecure' | 'unsupported' | 'video_unsupported' | 'too_large' | 'denied';

export class CopyAssetError extends Error {
  constructor(readonly reason: CopyRefusal) {
    super(reason);
  }
}

type ClipboardItemCtor = {
  new (items: Record<string, Blob | Promise<Blob>>): ClipboardItem;
  supports?: (type: string) => boolean;
};

export type CopyEnv = {
  secure: boolean;
  clipboard?: Pick<Clipboard, 'writeText' | 'write'>;
  ClipboardItem?: ClipboardItemCtor;
  /** Browsers only accept PNG images; any other image is re-encoded first. */
  toPng: (blob: Blob) => Promise<Blob>;
};

export function browserCopyEnv(): CopyEnv {
  return {
    secure: typeof window !== 'undefined' && window.isSecureContext,
    clipboard: typeof navigator !== 'undefined' ? navigator.clipboard : undefined,
    ClipboardItem: typeof ClipboardItem !== 'undefined' ? ClipboardItem as ClipboardItemCtor : undefined,
    toPng: encodePng,
  };
}

/** Whether the button can work at all, decided before any download. */
export function copyRefusal(kind: CopyKind, mime: string, env: CopyEnv): CopyRefusal | null {
  if (!env.secure || !env.clipboard) return 'insecure';
  if (kind === 'text') return null;
  if (!env.ClipboardItem) return 'unsupported';
  // Video is not a clipboard type in current browsers; only offer it where the
  // browser says it accepts this exact type.
  if (kind === 'video' && !env.ClipboardItem.supports?.(mime)) return 'video_unsupported';
  return null;
}

export async function copyAsset(
  kind: CopyKind,
  mime: string,
  loadBlob: () => Promise<Blob>,
  env: CopyEnv,
): Promise<void> {
  const refusal = copyRefusal(kind, mime, env);
  if (refusal) throw new CopyAssetError(refusal);
  // Our own refusal while preparing the payload, distinct from the browser's.
  let preparation: CopyAssetError | null = null;
  const prepare = async (): Promise<Blob> => {
    const blob = await loadBlob();
    if (kind === 'text') {
      if (blob.size > COPY_TEXT_MAX_BYTES) {
        preparation = new CopyAssetError('too_large');
        throw preparation;
      }
      return new Blob([await blob.text()], { type: 'text/plain' });
    }
    return kind === 'image' && blob.type !== 'image/png' ? env.toPng(blob) : blob;
  };
  try {
    if (!env.ClipboardItem) {
      // Text only reaches here: nothing better than writing once it is loaded.
      await env.clipboard!.writeText(await (await prepare()).text());
      return;
    }
    // WebKit only honours a write started within the click, so the item is
    // handed over at once and resolves once the download is done.
    const type = kind === 'text' ? 'text/plain' : kind === 'image' ? 'image/png' : mime;
    await env.clipboard!.write([new env.ClipboardItem({ [type]: prepare() })]);
  } catch (error) {
    if (preparation) throw preparation;
    if (error instanceof CopyAssetError) throw error;
    throw new CopyAssetError('denied');
  }
}

async function encodePng(blob: Blob): Promise<Blob> {
  const bitmap = await createImageBitmap(blob);
  const canvas = document.createElement('canvas');
  canvas.width = bitmap.width;
  canvas.height = bitmap.height;
  canvas.getContext('2d')!.drawImage(bitmap, 0, 0);
  bitmap.close();
  return new Promise((resolve, reject) => {
    canvas.toBlob(png => (png ? resolve(png) : reject(new Error('PNG encoding failed'))), 'image/png');
  });
}
