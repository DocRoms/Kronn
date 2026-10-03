import { useState, useRef } from 'react';
import type { ToastFn } from '../../hooks/useToast';
import type { RefObject } from 'react';
import { mcps as mcpsApi } from '../../lib/api';
import { userError } from '../../lib/userError';
import type { ApiAuthKind, ApiEndpoint, McpServer } from '../../types/generated';

// 0.8.6 (#33) — Custom plugin import/export, clipboard-JSON MVP.
//
// EXPORT contract: spec-only. We DELIBERATELY exclude all secret values
// (fields[].value = ''). Sharing a plugin = sharing its shape, never its
// credentials. The recipient fills env on their end via "Edit env".
type CustomPluginExport = {
  name: string;
  base_url: string;
  description: string;
  docs_url: string | null;
  fields: Array<{ label: string; value: string }>;
  endpoints: ApiEndpoint[];
  auth: ApiAuthKind;
};

interface UseCustomApiPortabilityArgs {
  t: (key: string, ...args: (string | number)[]) => string;
  toast: ToastFn;
  refetchMcps: () => void;
  /** `resetAddMcp` resets fields owned by 3 sibling hooks (registry +
   *  custom form + this one), so it can only be built by the composer
   *  AFTER all 3 have returned their setters. A ref breaks that cycle:
   *  the composer keeps it pointed at the latest `resetAddMcp` closure,
   *  and these handlers call it long after render (on submit), by
   *  which point the ref is always populated. */
  resetAddMcpRef: RefObject<() => void>;
}

/** Custom API plugin JSON import/export — the "portable spec" side of
 *  the Custom API/JSON editor. Covers the paste/upload import form and
 *  the copy-to-clipboard/download-file export modal. Split out of the
 *  monolithic `useMcpPageState` (KT-830) to stay under the page's
 *  per-file line budget. Distinct from `portabilityMode` (bulk plugin
 *  bundle export/import via `PluginPortabilityModal`, owned by
 *  `usePluginListState`). */
export function useCustomApiPortability({ t, toast, refetchMcps, resetAddMcpRef }: UseCustomApiPortabilityArgs) {
  // 0.8.6 (#33) — Import-from-JSON state. Inline textarea, no modal,
  // mirrors the Add-MCP "registry → form" flip pattern. Set when the
  // user clicks the "Importer depuis JSON" tile; cleared on reset.
  const [importJsonText, setImportJsonText] = useState('');
  const [importJsonError, setImportJsonError] = useState<string | null>(null);
  const [importJsonLoading, setImportJsonLoading] = useState(false);
  // 0.8.6 (#33 fix 2026-05-21) — Tauri's webview silently swallows
  // `navigator.clipboard.writeText` in some configs (no permission +
  // no exception). Pre-fix `handleExportCustomPlugin` looked dead :
  // no toast, no fallback, "STRICTEMENT rien" reported live by user.
  // Now we ALWAYS render an inline export modal with the JSON in a
  // readonly textarea (auto-selected on open) + a best-effort copy
  // button. Even if the clipboard fails the user can ctrl+C the
  // pre-selected text.
  const [exportPayload, setExportPayload] = useState<{ name: string; json: string } | null>(null);
  const [exportCopyState, setExportCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const exportTextareaRef = useRef<HTMLTextAreaElement | null>(null);

  const buildCustomPluginExport = (server: McpServer): CustomPluginExport | null => {
    if (!server.api_spec) return null;
    const spec = server.api_spec;
    return {
      name: server.name,
      base_url: spec.base_url,
      description: server.description,
      docs_url: spec.docs_url ?? null,
      fields: (spec.config_keys ?? []).map(ck => ({ label: ck.label, value: '' })),
      endpoints: spec.endpoints ?? [],
      auth: spec.auth ?? 'None',
    };
  };

  // Try the modern clipboard API, then fall back to the legacy
  // execCommand path (still works in Tauri webviews where the
  // permission-gated clipboard rejects silently). Returns whether
  // the write actually landed somewhere.
  const writeToClipboard = async (text: string): Promise<boolean> => {
    if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch (e) {
        console.warn('navigator.clipboard.writeText failed:', e);
      }
    }
    // Legacy fallback: stage a hidden textarea + execCommand('copy').
    // Doesn't need permissions and works in old Chrome / webviews.
    try {
      const el = document.createElement('textarea');
      el.value = text;
      el.setAttribute('readonly', '');
      el.style.position = 'absolute';
      el.style.left = '-9999px';
      document.body.appendChild(el);
      el.select();
      const ok = document.execCommand('copy');
      document.body.removeChild(el);
      return ok;
    } catch (e) {
      console.warn('execCommand("copy") failed:', e);
      return false;
    }
  };

  const handleExportCustomPlugin = async (server: McpServer) => {
    const payload = buildCustomPluginExport(server);
    if (!payload) {
      toast(t('mcp.custom.exportError'), 'error');
      return;
    }
    const json = JSON.stringify(payload, null, 2);
    // Pre-fix : a silent clipboard call left the user with no signal
    // at all. Now we surface the JSON in an inline modal regardless
    // of clipboard outcome, AND attempt to copy in the background.
    setExportPayload({ name: payload.name, json });
    const ok = await writeToClipboard(json);
    setExportCopyState(ok ? 'copied' : 'failed');
    if (ok) {
      toast(t('mcp.custom.copied'), 'success');
    }
  };

  const closeExportModal = () => {
    setExportPayload(null);
    setExportCopyState('idle');
  };

  const handleExportRetryCopy = async () => {
    if (!exportPayload) return;
    const ok = await writeToClipboard(exportPayload.json);
    setExportCopyState(ok ? 'copied' : 'failed');
    if (ok) toast(t('mcp.custom.copied'), 'success');
  };

  // 0.8.6 (#63) — Path B file download. Blob the JSON, trigger a
  // download. Filename sanitised similar to the backend's
  // `sanitize_filename` helper. Works in Tauri webview (no Auth headers
  // required, no server round-trip).
  const handleExportDownloadFile = (name: string, json: string) => {
    const safeName = name
      .replace(/[^A-Za-z0-9_-]+/g, '-')
      .replace(/-+/g, '-')
      .replace(/^-|-$/g, '');
    const filename = `${safeName || 'plugin'}.kronn-plugin.json`;
    try {
      const blob = new Blob([json], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = filename;
      a.style.display = 'none';
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      // Defer revoke so the browser's download UI has time to grab the blob.
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      toast(t('mcp.custom.downloaded', filename), 'success');
    } catch (e) {
      console.warn('download blob failed:', e);
      toast(t('mcp.custom.downloadFailed'), 'error');
    }
  };

  // Parse + validate an import payload. Returns the typed spec or a
  // user-facing error message. Validation mirrors the backend contract
  // for `custom_spec`: name + base_url are required; everything else is
  // optional and defaults to a sane empty value.
  const parseCustomPluginImport = (raw: string): { ok: true; spec: CustomPluginExport } | { ok: false; error: string } => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return { ok: false, error: t('mcp.custom.importErrorParse') };
    }
    if (!parsed || typeof parsed !== 'object') {
      return { ok: false, error: t('mcp.custom.importErrorShape') };
    }
    const p = parsed as Record<string, unknown>;
    const name = typeof p.name === 'string' ? p.name.trim() : '';
    const base_url = typeof p.base_url === 'string' ? p.base_url.trim() : '';
    if (!name) return { ok: false, error: t('mcp.custom.importErrorName') };
    if (!base_url) return { ok: false, error: t('mcp.custom.importErrorBaseUrl') };
    const fields = Array.isArray(p.fields)
      ? p.fields
          .filter((f): f is { label: unknown } => !!f && typeof f === 'object')
          .map(f => ({
            label: typeof (f as { label?: unknown }).label === 'string' ? (f as { label: string }).label : '',
            // 0.8.6 contract: values are NEVER imported. Even if an
            // ill-meaning peer included them, we wipe to '' to avoid
            // silently planting their creds in the user's env.
            value: '',
          }))
          .filter(f => f.label.trim() !== '')
      : [];
    const endpoints = Array.isArray(p.endpoints)
      ? (p.endpoints as ApiEndpoint[]).filter(e => !!e && typeof (e as ApiEndpoint).path === 'string')
      : [];
    // ApiAuthKind is a discriminated union: bare 'None' OR a single-key
    // object whose key names the variant ({Bearer:{…}}, {TokenExchange:{…}}, …).
    // Accept whatever shape the import advertises if it matches the wire
    // contract; default to 'None' otherwise. Backend re-validates on POST.
    const isValidAuth = (v: unknown): v is ApiAuthKind => {
      if (v === 'None') return true;
      if (!v || typeof v !== 'object' || Array.isArray(v)) return false;
      const keys = Object.keys(v as Record<string, unknown>);
      if (keys.length !== 1) return false;
      const allowedVariants = ['Bearer', 'ApiKeyHeader', 'ApiKeyQuery', 'Basic', 'BasicApiKey', 'OAuth2', 'TokenExchange'];
      return allowedVariants.includes(keys[0]);
    };
    const auth: ApiAuthKind = isValidAuth(p.auth) ? p.auth : 'None';
    return {
      ok: true,
      spec: {
        name,
        base_url,
        description: typeof p.description === 'string' ? p.description : '',
        docs_url: typeof p.docs_url === 'string' && p.docs_url.trim() !== '' ? p.docs_url : null,
        fields,
        endpoints,
        auth,
      },
    };
  };

  // 0.8.6 (#63) — Path B file upload. User picks a .json file ; we
  // read it client-side and POST as JSON. Same secret-strip contract
  // as the paste-textarea path applies server-side.
  const handleImportFromFile = async (file: File) => {
    setImportJsonError(null);
    let text: string;
    try {
      text = await file.text();
    } catch (e) {
      console.warn('read file failed:', e);
      setImportJsonError(t('mcp.custom.importFileReadFailed'));
      return;
    }
    setImportJsonText(text);
    const res = parseCustomPluginImport(text);
    if (!res.ok) {
      setImportJsonError(res.error);
      return;
    }
    setImportJsonLoading(true);
    try {
      await mcpsApi.importPluginFile({
        name: res.spec.name,
        base_url: res.spec.base_url,
        description: res.spec.description,
        docs_url: res.spec.docs_url,
        fields: res.spec.fields,
        endpoints: res.spec.endpoints,
        auth: res.spec.auth,
      });
      toast(t('mcp.custom.imported', res.spec.name), 'success');
      resetAddMcpRef.current();
      refetchMcps();
    } catch (e) {
      console.warn('Failed to import Custom API from file:', e);
      setImportJsonError(userError(e));
    } finally {
      setImportJsonLoading(false);
    }
  };

  const handlePasteImportJson = async () => {
    try {
      const txt = await navigator.clipboard.readText();
      setImportJsonText(txt);
      setImportJsonError(null);
    } catch (e) {
      console.warn('Clipboard read failed:', e);
      toast(t('mcp.custom.importPasteUnavailable'), 'error');
    }
  };

  const handleImportCustomPlugin = async () => {
    setImportJsonError(null);
    const res = parseCustomPluginImport(importJsonText);
    if (!res.ok) {
      setImportJsonError(res.error);
      return;
    }
    setImportJsonLoading(true);
    try {
      await mcpsApi.createConfig({
        server_id: 'api-custom',
        label: res.spec.name,
        env: {},
        args_override: null,
        is_global: false,
        project_ids: [],
        host_sync: 'None',
        custom_spec: {
          name: res.spec.name,
          base_url: res.spec.base_url,
          description: res.spec.description,
          docs_url: res.spec.docs_url,
          fields: res.spec.fields,
          endpoints: res.spec.endpoints,
          auth: res.spec.auth,
        },
      });
      toast(t('mcp.custom.imported', res.spec.name), 'success');
      resetAddMcpRef.current();
      refetchMcps();
    } catch (e) {
      console.warn('Failed to import Custom API:', e);
      setImportJsonError(userError(e));
    } finally {
      setImportJsonLoading(false);
    }
  };

  return {
    importJsonText, setImportJsonText, importJsonError, setImportJsonError, importJsonLoading,
    exportPayload, exportCopyState, exportTextareaRef,
    handleExportCustomPlugin, closeExportModal, handleExportRetryCopy, handleExportDownloadFile,
    handleImportFromFile, handlePasteImportJson, handleImportCustomPlugin,
  };
}
