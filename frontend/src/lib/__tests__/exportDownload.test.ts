import { describe, it, expect, vi, beforeEach } from 'vitest';
import { exportAndDownload } from '../exportDownload';

describe('exportAndDownload', () => {
  const t = (k: string) => k;
  beforeEach(() => {
    URL.createObjectURL = vi.fn(() => 'blob:x');
    URL.revokeObjectURL = vi.fn();
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  });

  it('shows a toast for the export warning header', async () => {
    const toast = vi.fn();
    await exportAndDownload(async () => ({ blob: new Blob(['z']), warning: 'no-recovery-passphrase' }), toast, t);
    expect(toast).toHaveBeenCalledWith('config.exportNoRecoveryWarning', 'error');
  });

  it('shows no toast without a warning', async () => {
    const toast = vi.fn();
    await exportAndDownload(async () => ({ blob: new Blob(['z']), warning: null }), toast, t);
    expect(toast).not.toHaveBeenCalled();
  });
});
