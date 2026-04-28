// 0.7.0 UX pass — minimal helper to trigger a browser file download
// from a Blob received via fetch. The browser doesn't expose a single
// API for this; we have to fabricate an `<a download>` and click it.
//
// Caller pattern :
//   const { filename, blob } = await api.exportWorkflow(id);
//   triggerDownload(filename, blob);

export function triggerDownload(filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  // Free the blob URL on the next tick — `click()` is sync but the
  // browser still needs the URL alive for the download to start.
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
