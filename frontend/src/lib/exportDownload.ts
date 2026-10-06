// Settings → Export: download the archive and say what it lacks (KT-1007):
// encrypted secrets exported without recovery data cannot be read elsewhere.
import type { ToastFn } from '../hooks/useToast';

type T = (key: string, ...args: (string | number)[]) => string;

const WARNINGS: Record<string, string> = {
  'no-recovery-passphrase': 'config.exportNoRecoveryWarning',
  'key-locked': 'config.exportKeyLockedWarning',
  'recovery-not-bundled': 'config.exportRecoveryWarning',
};

export async function exportAndDownload(
  exportData: () => Promise<{ blob: Blob; warning: string | null }>,
  toast: ToastFn,
  t: T,
): Promise<void> {
  try {
    const { blob, warning } = await exportData();
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `kronn-export-${new Date().toISOString().slice(0, 10)}.zip`;
    a.click();
    URL.revokeObjectURL(url);
    if (warning) toast(t(WARNINGS[warning] ?? 'config.exportRecoveryWarning'), 'error');
  } catch (err) {
    toast(err instanceof Error ? err.message : String(err), 'error');
  }
}
