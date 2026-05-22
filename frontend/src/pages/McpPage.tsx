import { useState, useRef, useEffect } from 'react';
import { mcps as mcpsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useToast } from '../hooks/useToast';
import { userError } from '../lib/userError';
import { isHiddenPath } from '../lib/constants';
import type { AgentType, ApiAuthKind, ApiEndpoint, Project, McpConfigDisplay, McpDefinition, McpOverview, HostSyncMode, ApiSpec, CustomApiPayload, McpServer } from '../types/generated';
import { CustomApiAiHelper } from '../components/CustomApiAiHelper';
import { Dropdown } from '../components/Dropdown';
import {
  Puzzle, Plus, Trash2, Eye, Check, RefreshCw, Square, CheckSquare,
  X, Key, Pencil, FileText, ExternalLink, Save, Search, ArrowDownAZ, ArrowDownZA,
  Plug, Globe, Info, Sparkles, Upload, Download,
} from 'lucide-react';
import { HostSyncChip } from '../components/HostSyncChip';
import { HostSyncPreview } from '../components/HostSyncPreview';

/** Derive plugin kind from transport + tags + api_spec presence.
 *
 *  0.8.6 phase 4 — `cli` added : plugins that speak MCP BUT shell out
 *  to a local CLI binary (Fastly via `fastly-mcp`, GitLab via `glab
 *  mcp serve`). From the user's install standpoint they have the
 *  same prereq as a CLI agent (binary on the host), so we surface
 *  them as a separate category instead of lumping them with pure-MCP
 *  servers. Detection : the registry entry carries a `cli` tag.
 *
 *  `cli` is checked FIRST so it wins over both `api`/`hybrid` detection
 *  — e.g. a future CLI wrapper that ALSO exposes a REST API stays
 *  bucketed as CLI (the prereq is what matters to the user). */
type PluginKind = 'mcp' | 'api' | 'hybrid' | 'cli';
function pluginKind(m: { transport: McpDefinition['transport']; api_spec?: ApiSpec | null; tags?: string[] }): PluginKind {
  const hasApi = !!m.api_spec;
  const hasCliTag = Array.isArray(m.tags) && m.tags.includes('cli');
  // McpTransport is a discriminated union; the API-only sentinel is the
  // string literal "ApiOnly" (not a { tag: ... } object).
  const isApiOnly = (m.transport as unknown) === 'ApiOnly';
  if (hasCliTag) return 'cli';
  if (isApiOnly) return 'api';
  if (hasApi) return 'hybrid';
  return 'mcp';
}

/**
 * Compact "what kind of plugin is this" badge — shown on each installed
 * config card so the user can tell at a glance whether the plugin
 * surfaces tools via MCP (synced to `.mcp.json`), via REST API (injected
 * in the agent's system prompt), or both. Avoids the trap where a user
 * sees a `🌐 CLI local` chip on an API-only plugin and assumes it's
 * being written to their host config files (it isn't — only MCP
 * transports are synced; API-only plugins live in prompts).
 */
function PluginKindBadge({ kind }: { kind: PluginKind }) {
  const meta = kind === 'cli'
    ? { label: '⌨ CLI wrapper', tooltip: 'Wraps a local CLI binary. The agent talks to it via MCP, BUT the binary (fastly, glab, …) MUST be installed on the host first. Bucketed separately from pure MCP because the install prereq is different.' }
    : kind === 'api'
    ? { label: '🌐 API', tooltip: 'API plugin — endpoints injected in the agent\'s system prompt (curl). Not synced to ~/.claude.json or other CLI config files.' }
    : kind === 'hybrid'
    ? { label: '🔌🌐 MCP + API', tooltip: 'Hybrid plugin — both an MCP transport (synced to .mcp.json) and a REST API (injected in prompt). The "Portée CLI locale" toggle only affects the MCP side.' }
    : { label: '🔌 MCP', tooltip: 'MCP plugin — tools synced to `.mcp.json` and friends. The "Portée CLI locale" toggle controls whether this entry is mirrored into ~/.claude.json, ~/.gemini/settings.json, etc.' };
  return (
    <span
      className="mcp-scope-badge"
      title={meta.tooltip}
      style={{ fontSize: '0.75em' }}
    >
      {meta.label}
    </span>
  );
}
import { MatrixText } from '../components/MatrixText';
import './McpPage.css';

const slugify = (label: string) => label.toLowerCase().replace(/[^a-z0-9]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');

/** Placeholder hints for common MCP env vars — helps non-dev users understand what to enter */
const ENV_PLACEHOLDERS: Record<string, string> = {
  // Atlassian / Jira / Confluence
  JIRA_URL: 'https://your-company.atlassian.net',
  JIRA_USERNAME: 'prenom.nom@company.com',
  JIRA_API_TOKEN: 'ATATT3x... (from id.atlassian.com)',
  CONFLUENCE_URL: 'https://your-company.atlassian.net/wiki',
  CONFLUENCE_USERNAME: 'prenom.nom@company.com',
  CONFLUENCE_API_TOKEN: 'ATATT3x... (same as Jira token)',
  // GitHub
  GITHUB_PERSONAL_ACCESS_TOKEN: 'ghp_xxxxxxxxxxxx',
  GITHUB_TOKEN: 'ghp_xxxxxxxxxxxx',
  // GitLab
  GITLAB_PERSONAL_ACCESS_TOKEN: 'glpat-xxxxxxxxxxxx',
  GITLAB_URL: 'https://gitlab.com',
  // Slack
  SLACK_BOT_TOKEN: 'xoxb-xxxxxxxxxxxx',
  SLACK_TEAM_ID: 'T0XXXXXXX',
  // Microsoft 365 (optional — leave empty to use default app)
  MS365_MCP_TENANT_ID: 'e59fa28a-... (ID annuaire, optionnel)',
  MS365_MCP_CLIENT_ID: '2ac5e4f9-... (ID application, optionnel)',
  // MongoDB
  MDB_MCP_CONNECTION_STRING: 'mongodb+srv://user:pass@cluster.mongodb.net/db',
  MDB_MCP_ATLAS_CLIENT_ID: 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx',
  MDB_MCP_ATLAS_CLIENT_SECRET: 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx',
  // Qdrant
  QDRANT_URL: 'http://localhost:6333',
  COLLECTION_NAME: 'my-collection',
  EMBEDDING_MODEL: 'sentence-transformers/all-MiniLM-L6-v2',
  // Perplexity
  PERPLEXITY_API_KEY: 'pplx-xxxxxxxxxxxx',
  // Linear
  LINEAR_API_KEY: 'lin_api_xxxxxxxxxxxx',
  // Notion
  NOTION_API_KEY: 'ntn_xxxxxxxxxxxx',
  // OpenAI
  OPENAI_API_KEY: 'sk-xxxxxxxxxxxx',
  // Anthropic
  ANTHROPIC_API_KEY: 'sk-ant-xxxxxxxxxxxx',
  // Google
  GOOGLE_API_KEY: 'AIzaXXXXXXXXXX',
  // Sentry
  SENTRY_AUTH_TOKEN: 'sntrys_xxxxxxxxxxxx',
  SENTRY_ORG: 'your-organization-slug',
  SENTRY_PROJECT: 'your-project-slug',
  // Brave
  BRAVE_API_KEY: 'BSA_xxxxxxxxxxxx',
  // Exa
  EXA_API_KEY: 'exa-xxxxxxxxxxxx',
  // Redis
  REDIS_URL: 'redis://localhost:6379',
  // PostgreSQL
  DATABASE_URL: 'postgresql://user:pass@localhost:5432/db',
  POSTGRES_CONNECTION_STRING: 'postgresql://user:pass@localhost:5432/db',
  // Chartbeat (API plugin)
  CHARTBEAT_API_KEY: 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx (32-char key from chartbeat.com account)',
  CHARTBEAT_HOST: 'domain.tld (the site tracked in Chartbeat)',
  // Adobe Analytics (OAuth2 S2S) — placeholders intentionally don't match Adobe's
  // real secret prefixes (p8e-, s8e-) so GitHub's secret-scanner push protection
  // doesn't flag this file when the repo is pushed.
  ADOBE_CLIENT_ID: 'your-adobe-client-id (from Adobe Developer Console project)',
  ADOBE_CLIENT_SECRET: 'your-adobe-client-secret (generated in the same project)',
  // Google Programmable Search — ditto, avoid the AIza prefix that Google real keys use.
  GOOGLE_SEARCH_API_KEY: 'your-google-cloud-api-key (from console.cloud.google.com → APIs & Credentials)',
  // Generic patterns
  API_KEY: 'your-api-key',
  API_TOKEN: 'your-api-token',
  API_SECRET: 'your-api-secret',
  BASE_URL: 'https://api.example.com',
};

/** Turn plain text with URLs into React nodes with clickable links */
function linkify(text: string): React.ReactNode[] {
  const urlRe = /(https?:\/\/[^\s)]+)/g;
  const parts = text.split(urlRe);
  return parts.map((part, i) =>
    urlRe.test(part)
      ? <a key={i} href={part} target="_blank" rel="noopener noreferrer" className="mcp-secrets-token-link" style={{ display: 'inline' }}>{part}</a>
      : part
  );
}

interface McpPageProps {
  projects: Project[];
  mcpOverview: McpOverview;
  mcpRegistry: McpDefinition[];
  refetchMcps: () => void;
  initialSelectedConfigId?: string | null;
  /** Installed agent types — threaded through to the Custom API AI
   *  helper bubble so the user can pick which local agent runs the
   *  helper conversation. Optional: when empty, the helper trigger
   *  surfaces a "no agents installed" message instead of opening. */
  installedAgentTypes?: AgentType[];
  /** Backend output language (Settings → Output language) — used as the
   *  agent's reply language inside the helper. Falls back to 'fr' when
   *  missing, mirroring the Dashboard-level default. */
  configLanguage?: string;
}

export function McpPage({ projects, mcpOverview, mcpRegistry, refetchMcps, initialSelectedConfigId, installedAgentTypes, configLanguage }: McpPageProps) {
  const { t } = useT();
  const { toast } = useToast();
  const detailRef = useRef<HTMLDivElement>(null);
  const [editingLabelId, setEditingLabelId] = useState<string | null>(null);
  const [editingLabelText, setEditingLabelText] = useState('');
  const [showAddMcp, setShowAddMcp] = useState(false);
  const [addMcpSearch, setAddMcpSearch] = useState('');
  // 0.8.6 phase 4 — top-level type filter (audit feedback 2026-05-22).
  // Lets the user narrow the discovery dropdown to a specific TRANSPORT
  // kind (MCP / API / CLI). Defaults to 'all' so the existing behaviour
  // (every plugin visible) is preserved when nothing is selected. The
  // category-by-tag grouping below continues to apply within the
  // filtered subset.
  const [addMcpKindFilter, setAddMcpKindFilter] = useState<'all' | PluginKind>('all');
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [addMcpSelected, setAddMcpSelected] = useState<string | null>(null);
  const [addMcpLabel, setAddMcpLabel] = useState('');
  const [addMcpEnv, setAddMcpEnv] = useState<Record<string, string>>({});
  const [addMcpGlobal, setAddMcpGlobal] = useState(false);
  const [addVisibleFields, setAddVisibleFields] = useState<Set<string>>(new Set());
  const addMcpRef = useRef<HTMLDivElement>(null);
  // Custom API form state. Only meaningful when addMcpSelected === 'api-custom'.
  // The shape mirrors `CustomApiPayload` in generated.ts so submit can
  // forward it as-is via `custom_spec`.
  const [customName, setCustomName] = useState('');
  const [customBaseUrl, setCustomBaseUrl] = useState('');
  const [customDescription, setCustomDescription] = useState('');
  const [customDocsUrl, setCustomDocsUrl] = useState('');
  const [customFields, setCustomFields] = useState<Array<{ label: string; value: string }>>([
    { label: '', value: '' },
  ]);
  // 0.8.6 — endpoints declared at creation time. Empty by default so
  // pre-existing flows (no endpoints → manual ApiCall path) stay
  // identical. The AI helper populates this array via `KRONN:APPLY`
  // after a WebFetch of `docs_url`; the user can also add rows
  // manually. Cf. [[project_endpoints_autodiscovery_0_8_6]].
  const [customEndpoints, setCustomEndpoints] = useState<ApiEndpoint[]>([]);
  // 0.8.6 — Edit-existing-Custom-plugin flow. When non-null, the form
  // is in edit mode: pre-filled from the existing plugin's spec, submit
  // goes to PUT instead of POST. Cleared on reset / form-close.
  const [editingCustomServerId, setEditingCustomServerId] = useState<string | null>(null);
  // 0.8.6 — the config id whose env to PATCH on save when in edit mode.
  // Captured at edit-button click time alongside the server_id so the
  // submit handler can ALSO update credential values without forcing
  // the user to leave for the env drawer. Single-form UX for both
  // spec and values, on user request 2026-05-20.
  const [editingCustomConfigId, setEditingCustomConfigId] = useState<string | null>(null);
  // 0.8.6 — `editingStoredEnvKeys` removed 2026-05-20 when the edit
  // form's value column was replaced by a static "→ Édite via Éditer
  // les secrets" passive text. The per-field "•••• stocké" placeholder
  // is no longer needed (no input, no need to hint "still stored").
  // Orphan-env warning now lives on the env-edit drawer side (where it
  // belongs architecturally) rather than this spec-edit form.
  //
  // 0.8.6 (2026-05-20) — per-row reveal toggle for the unified edit
  // form. Indexed by row position (not env_key) because labels can
  // be renamed mid-edit and the user might want to reveal the value
  // before/after a rename. Set on `setCustomFieldsVisible`, cleared
  // on `resetAddMcp`.
  const [customFieldsVisible, setCustomFieldsVisible] = useState<Set<number>>(new Set());
  // 0.8.6 — Custom plugin auth state. MVP exposes 3 variants out of
  // the 7 supported by the runtime: None (default), Bearer (simple
  // static token), TokenExchange (Didomi-shape: POST creds → access
  // token → Bearer). The other 4 (ApiKeyQuery / ApiKeyHeader / Basic /
  // BasicApiKey / OAuth2) come in 0.8.6 Layer A — they all work in
  // the runtime, just no UI yet. Cf. [[project_custom_plugin_auth_0_8_7]].
  const [customAuth, setCustomAuth] = useState<ApiAuthKind>('None');
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
  // Edit secrets
  const [editingEnvId, setEditingEnvId] = useState<string | null>(null);
  const [editingEnv, setEditingEnv] = useState<Record<string, string>>({});
  const [editingEnvLoading, setEditingEnvLoading] = useState(false);
  const [visibleFields, setVisibleFields] = useState<Set<string>>(new Set());
  const [editingEnvError, setEditingEnvError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  // MCP context editor
  const [contextEditor, setContextEditor] = useState<{ projectId: string; projectName: string; slug: string; content: string } | null>(null);
  const [contextSaving, setContextSaving] = useState(false);
  // Search & detail panel
  const [mcpSearch, setMcpSearch] = useState('');
  const [mcpSort, setMcpSort] = useState<'az' | 'za'>(() => {
    try {
      const saved = localStorage.getItem('kronn:mcpSort');
      return saved === 'za' ? 'za' : 'az';
    } catch { return 'az'; }
  });
  useEffect(() => {
    try { localStorage.setItem('kronn:mcpSort', mcpSort); } catch { /* localStorage disabled (incognito / quota) — sort defaults to az on next load */ }
  }, [mcpSort]);
  const [selectedConfigId, setSelectedConfigId] = useState<string | null>(initialSelectedConfigId ?? null);

  // Open a specific config when navigated from another page (e.g. ProjectCard)
  useEffect(() => {
    if (initialSelectedConfigId) {
      setSelectedConfigId(initialSelectedConfigId);
    }
  }, [initialSelectedConfigId]);

  // Scroll to detail panel when a config is selected
  useEffect(() => {
    if (selectedConfigId && detailRef.current) {
      detailRef.current.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
  }, [selectedConfigId]);
  // "Show more" for project toggles per config
  const [expandedProjectLists, setExpandedProjectLists] = useState<Set<string>>(new Set());
  const PROJECT_TOGGLE_LIMIT = 10;

  // ── Handlers ──

  const handleSaveLabel = async (configId: string) => {
    if (!editingLabelText.trim()) return;
    try {
      await mcpsApi.updateConfig(configId, { label: editingLabelText.trim() });
      setEditingLabelId(null);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to save label:', e);
    }
  };

  const resetAddMcp = () => {
    setShowAddMcp(false);
    setAddMcpSelected(null);
    setAddMcpLabel('');
    setAddMcpEnv({});
    setAddMcpGlobal(false);
    setAddMcpSearch('');
    setCustomName('');
    setCustomBaseUrl('');
    setCustomDescription('');
    setCustomDocsUrl('');
    setCustomFields([{ label: '', value: '' }]);
    setCustomEndpoints([]);
    setEditingCustomServerId(null);
    setEditingCustomConfigId(null);
    setCustomAuth('None');
    setCustomFieldsVisible(new Set());
    setImportJsonText('');
    setImportJsonError(null);
    setImportJsonLoading(false);
  };

  /** 0.8.6 — Discriminated-union helpers for the auth picker. The
   *  Rust enum serializes as `"None"` for the bare variant and
   *  `{ Bearer: { env_key } }` for struct variants — typeof checks
   *  branch on the wire format. */
  const authKindOf = (a: ApiAuthKind): 'None' | 'Bearer' | 'TokenExchange' | 'Other' => {
    if (a === 'None') return 'None';
    if (typeof a === 'object' && 'Bearer' in a) return 'Bearer';
    if (typeof a === 'object' && 'TokenExchange' in a) return 'TokenExchange';
    return 'Other';  // ApiKeyQuery / ApiKeyHeader / Basic / BasicApiKey / OAuth2 — exposed in 0.8.6 Layer A
  };
  const setAuthKindBy = (kind: 'None' | 'Bearer' | 'TokenExchange') => {
    if (kind === 'None') setCustomAuth('None');
    else if (kind === 'Bearer') setCustomAuth({ Bearer: { env_key: '' } });
    else setCustomAuth({
      TokenExchange: {
        endpoint: '',
        method: 'POST',
        body_template: {},
        body_format: 'Json',
        token_jsonpath: '$.access_token',
        ttl_seconds: 3600,
        inject: 'BearerHeader',
        creds_env_keys: [],
      },
    });
  };

  /** Slugify a Custom plugin field label into its UPPER_SNAKE env key.
   *  MUST stay in lockstep with the backend (`backend/src/api/mcps.rs:216`)
   *  so the "value is stored" hint detection works correctly. Algo:
   *  ASCII-alnum → upper, anything else → single `_`, trim trailing `_`,
   *  fallback "FIELD" if empty. */
  const slugEnvKey = (label: string): string => {
    let out = '';
    let prevUnderscore = true;
    for (const ch of label) {
      if (/[a-zA-Z0-9]/.test(ch)) {
        out += ch.toUpperCase();
        prevUnderscore = false;
      } else if (!prevUnderscore) {
        out += '_';
        prevUnderscore = true;
      }
    }
    out = out.replace(/_+$/, '');
    return out || 'FIELD';
  };

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

  // 0.8.6 (#63) — Path B file upload. User picks a .json file ; we
  // read it client-side and POST as JSON. Same secret-strip contract
  // as the paste-textarea path applies server-side.
  const handleImportFromFile = async (file: File) => {
    setImportJsonError(null);
    let text = '';
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
      resetAddMcp();
      refetchMcps();
    } catch (e) {
      console.warn('Failed to import Custom API from file:', e);
      setImportJsonError(userError(e));
    } finally {
      setImportJsonLoading(false);
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
      resetAddMcp();
      refetchMcps();
    } catch (e) {
      console.warn('Failed to import Custom API:', e);
      setImportJsonError(userError(e));
    } finally {
      setImportJsonLoading(false);
    }
  };

  const handleAddMcpFromRegistry = async () => {
    if (!addMcpSelected) return;
    // Custom API branch: forward the freeform form as `custom_spec` instead
    // of env-keys. Validation mirrors the backend (name + base_url required)
    // so the user sees the error before the round-trip.
    if (addMcpSelected === 'api-custom') {
      if (!customName.trim()) {
        toast(t('mcp.custom.errorName'), 'error');
        return;
      }
      if (!customBaseUrl.trim()) {
        toast(t('mcp.custom.errorBaseUrl'), 'error');
        return;
      }
      // 0.8.6 — Edit-existing branch. The form is reused for both
      // create (POST /api/mcps/configs) and edit (PUT /api/mcps/custom/:id).
      // The Edit button on a custom plugin row sets `editingCustomServerId`
      // and pre-fills the form. On submit, we route to the right endpoint.
      // The encrypted env per-config is NOT touched in edit mode — the
      // user uses the existing "edit env" drawer for that.
      if (editingCustomServerId) {
        // 0.8.6 fix 2026-05-20 : capture the name BEFORE `resetAddMcp`
        // clears `customName` to '' — otherwise the success toast read
        // an empty name post-reset and rendered `API «  » mise à jour`
        // (visually nothing). Same defensive capture for the create
        // path below, in case future refactors reorder.
        const savedName = customName.trim();
        const filteredFields = customFields.filter(f => f.label.trim() !== '');
        try {
          // Step 1 — update the spec (name / base_url / docs_url /
          // fields[].label / endpoints / auth). Server row touched here.
          const updateResp = await mcpsApi.updateCustomSpec(editingCustomServerId, {
            name: savedName,
            base_url: customBaseUrl.trim(),
            description: customDescription.trim(),
            docs_url: customDocsUrl.trim() || null,
            fields: filteredFields,
            endpoints: customEndpoints.filter(e => e.path.trim() !== ''),
            auth: customAuth,
          });
          // 0.8.6 (#60) — detect orphan env keys left behind by a rename
          // / removal across all OTHER configs of this server (the
          // current config's env gets wholesale-replaced in step 2 so
          // its orphans clean up automatically, but multi-project configs
          // need an explicit cleanup pass).
          const orphanKeys = updateResp.orphan_env_keys ?? [];
          // Step 2 — 0.8.6 unified edit : ALSO patch the encrypted env
          // for this config so a single form save persists BOTH spec
          // and credential values. Pre-fix the user typed values in
          // this form and they silently dropped (materialize_custom_server
          // only reads labels). Now we slug each label → env_key and
          // build the env map. Wholesale replacement = orphan slugs
          // (from a prior rename) get cleaned up automatically. Skip
          // empty values to avoid wiping the user's untouched fields
          // when the masked-pre-fill failed (revealSecrets glitch).
          if (editingCustomConfigId) {
            const newEnv: Record<string, string> = {};
            for (const f of filteredFields) {
              if (f.value !== '') {
                newEnv[slugEnvKey(f.label)] = f.value;
              }
            }
            try {
              await mcpsApi.updateConfig(editingCustomConfigId, { env: newEnv });
            } catch (envErr) {
              console.warn('Spec saved but env PATCH failed:', envErr);
              toast(t('mcp.custom.specSavedEnvFailed', userError(envErr)), 'error');
              resetAddMcp();
              refetchMcps();
              return;
            }
          }
          toast(t('mcp.custom.updated', savedName), 'success');
          // 0.8.6 (#60) — surface orphan-env warning AFTER the success
          // toast so the success path stays visible. The cleanup button
          // lives in the toast itself; if the user dismisses we keep
          // the warning visible until next render (or they reopen the
          // plugin and see the unchanged env_keys).
          if (orphanKeys.length > 0) {
            const proceed = confirm(
              t('mcp.custom.orphanEnvWarning', String(orphanKeys.length), orphanKeys.join(', ')),
            );
            if (proceed) {
              try {
                const cleanup = await mcpsApi.cleanupOrphanEnv(editingCustomServerId, orphanKeys);
                toast(
                  t('mcp.custom.orphanEnvCleaned',
                    String(cleanup.total_keys_removed),
                    String(cleanup.configs_updated)),
                  'success',
                );
              } catch (cleanupErr) {
                console.warn('cleanup_orphan_env failed:', cleanupErr);
                toast(t('mcp.custom.orphanEnvCleanFailed', userError(cleanupErr)), 'error');
              }
            }
          }
          resetAddMcp();
          refetchMcps();
        } catch (e) {
          console.warn('Failed to update Custom API:', e);
          toast(t('mcp.custom.error', userError(e)), 'error');
        }
        return;
      }
      try {
        await mcpsApi.createConfig({
          server_id: 'api-custom',
          label: addMcpLabel || customName,
          env: {},
          args_override: null,
          is_global: addMcpGlobal,
          project_ids: [],
          custom_spec: {
            name: customName.trim(),
            base_url: customBaseUrl.trim(),
            description: customDescription.trim(),
            docs_url: customDocsUrl.trim() || null,
            fields: customFields.filter(f => f.label.trim() !== ''),
            // 0.8.6 — drop blank-path rows the user added but never
            // filled (or the trailing "Add row" sentinel). Backend
            // does this too but client-side filter keeps the POST
            // payload lean and the "Empty endpoints?" hint on the
            // resulting plugin accurate.
            endpoints: customEndpoints.filter(e => e.path.trim() !== ''),
            auth: customAuth,
          },
        });
        resetAddMcp();
        refetchMcps();
        toast(t('mcp.custom.created', customName.trim()), 'success');
      } catch (e) {
        console.warn('Failed to add Custom API:', e);
        toast(t('mcp.custom.error', userError(e)), 'error');
      }
      return;
    }
    try {
      await mcpsApi.createConfig({
        server_id: addMcpSelected,
        label: addMcpLabel || mcpRegistry.find(m => m.id === addMcpSelected)?.name || 'New MCP',
        env: addMcpEnv,
        args_override: null,
        is_global: addMcpGlobal,
        project_ids: [],
      });
      resetAddMcp();
      refetchMcps();
    } catch (e) {
      console.warn('Failed to add MCP config:', e);
    }
  };

  const handleDeleteMcpConfig = async (configId: string) => {
    // Pre-fix: this fired on click with no confirm and no toast — operators
    // accidentally clicked the red Delete button (in a row of 3 actions on
    // the detail header) and lost their MCP config + linked projects +
    // env keys with no signal it had happened. Now an explicit native
    // confirm is required (mirrors the QP / project / skill delete flow)
    // and the result is toasted so success/failure is visible.
    const cfg = mcpOverview.configs.find(c => c.id === configId);
    const label = cfg?.label ?? configId;
    if (!confirm(t('mcp.deleteConfigConfirm', label))) return;
    try {
      await mcpsApi.deleteConfig(configId);
      refetchMcps();
      toast(t('mcp.deleteConfigSuccess', label), 'success');
    } catch (e) {
      console.warn('Failed to delete MCP config:', e);
      toast(t('mcp.deleteConfigError', userError(e)), 'error');
    }
  };

  const handleToggleConfigGlobal = async (config: McpConfigDisplay) => {
    try {
      await mcpsApi.updateConfig(config.id, { is_global: !config.is_global });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle global:', e);
    }
  };

  /** Update host_sync (CLI scope: None/GlobalOnly/MirrorAll). UX#2 — single
   *  source of edit; the SettingsPage section delegates here via deeplink. */
  const handleSetHostSync = async (configId: string, mode: HostSyncMode) => {
    try {
      await mcpsApi.updateConfig(configId, { host_sync: mode });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to set host_sync:', e);
    }
  };

  const handleToggleConfigProject = async (configId: string, projectId: string, currentlyLinked: boolean) => {
    const config = mcpOverview.configs.find(c => c.id === configId);
    if (!config) return;
    const newIds = currentlyLinked
      ? config.project_ids.filter(id => id !== projectId)
      : [...config.project_ids, projectId];
    try {
      await mcpsApi.setConfigProjects(configId, { project_ids: newIds });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle project:', e);
    }
  };

  const handleStartEditSecrets = async (configId: string): Promise<boolean> => {
    if (editingEnvId === configId) { setEditingEnvId(null); return false; }
    setEditingEnvLoading(true);
    setVisibleFields(new Set());
    setEditingEnvError(null);
    try {
      const entries = await mcpsApi.revealSecrets(configId);
      const env: Record<string, string> = {};
      entries.forEach(e => { env[e.key] = e.masked_value; });
      setEditingEnv(env);
      setEditingEnvId(configId);
      return true;
    } catch (e) {
      console.warn('Failed to load secrets:', e);
      // Enter edit mode with empty values so the user can re-enter tokens
      const cfg = mcpOverview.configs.find(c => c.id === configId);
      const env: Record<string, string> = {};
      cfg?.env_keys.forEach(k => { env[k] = ''; });
      setEditingEnv(env);
      setEditingEnvId(configId);
      setEditingEnvError(t('mcp.revealWarning'));
      return true;
    } finally {
      setEditingEnvLoading(false);
    }
  };

  const handleSaveSecrets = async () => {
    if (!editingEnvId) return;
    setEditingEnvLoading(true);
    try {
      // 0.8.6 — For Custom plugins, filter the env on save to ONLY the
      // env_keys the current spec declares. Orphans from a prior
      // rename get dropped here (the PATCH replaces the env wholesale,
      // so what we don't send disappears). For registry plugins, the
      // spec is immutable and stored env always matches → no-op.
      const cfg = mcpOverview.configs.find(c => c.id === editingEnvId);
      const server = cfg ? mcpOverview.servers.find(s => s.id === cfg.server_id) : null;
      const specKeys = server?.api_spec?.config_keys?.map(ck => ck.env_key) ?? [];
      const isCustom = cfg?.server_id.startsWith('custom-') ?? false;
      const envToSend: Record<string, string> = isCustom && specKeys.length > 0
        ? Object.fromEntries(Object.entries(editingEnv).filter(([k]) => specKeys.includes(k)))
        : editingEnv;
      await mcpsApi.updateConfig(editingEnvId, { env: envToSend });
      setEditingEnvId(null);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to save secrets:', e);
    } finally {
      setEditingEnvLoading(false);
    }
  };

  const toggleFieldVisibility = (key: string) => {
    setVisibleFields(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  };

  const handleOpenContext = async (projectId: string, projectName: string, configLabel: string) => {
    // Slugify the label (same algo as backend)
    const slug = slugify(configLabel);
    try {
      const entry = await mcpsApi.getContext(projectId, slug);
      setContextEditor({ projectId, projectName, slug, content: entry.content });
    } catch {
      // File might not exist yet — create with empty marker
      setContextEditor({ projectId, projectName, slug, content: `# ${configLabel} — Usage Context\n\n> Instructions for AI agents using **${configLabel}** in this project.\n> Edit this file with project-specific rules.\n\n## Rules\n\n` });
    }
  };

  const handleSaveContext = async () => {
    if (!contextEditor) return;
    setContextSaving(true);
    try {
      await mcpsApi.updateContext(contextEditor.projectId, contextEditor.slug, contextEditor.content);
      setContextEditor(null);
    } catch (e) {
      console.warn('Failed to save context:', e);
    } finally {
      setContextSaving(false);
    }
  };

  // ── Computed ──

  const { servers, configs } = mcpOverview;
  const totalConfigs = configs.length;
  const globalConfigs = configs.filter(c => c.is_global);

  const configsByServer = new Map<string, { serverId: string; serverName: string; configs: McpConfigDisplay[] }>();
  for (const c of configs) {
    const key = c.server_name || c.server_id;
    const existing = configsByServer.get(key) ?? { serverId: c.server_id, serverName: key, configs: [] };
    existing.configs.push(c);
    configsByServer.set(key, existing);
  }

  const configuredServerIds = new Set(configs.map(c => c.server_id));
  const availableRegistry = mcpRegistry.filter(m =>
    // Pinned separately at the top of the grid — keep it out of the
    // categorized list to avoid duplicating it under "Other".
    m.id !== 'api-custom' &&
    (!addMcpSearch || m.name.toLowerCase().includes(addMcpSearch.toLowerCase()) || m.tags.some(tag => tag.toLowerCase().includes(addMcpSearch.toLowerCase()))) &&
    // 0.8.6 phase 4 — type filter. `'all'` lets every plugin through,
    // otherwise we narrow to exactly that PluginKind. `'mcp'` is the
    // permissive default that ALSO matches `hybrid` (a hybrid plugin
    // is still primarily an MCP transport from the user's standpoint).
    (
      addMcpKindFilter === 'all'
        ? true
        : addMcpKindFilter === 'mcp'
          ? (pluginKind(m) === 'mcp' || pluginKind(m) === 'hybrid')
          : pluginKind(m) === addMcpKindFilter
    )
  );
  const selectedDef = mcpRegistry.find(m => m.id === addMcpSelected);
  // Whether the pinned Custom API tile should be visible. Hide it when the
  // user searches for something that doesn't match "custom" / "api" so the
  // tile doesn't fight for attention when they're clearly looking for
  // GitHub etc.
  const customApiVisible = !addMcpSearch
    || 'custom api'.includes(addMcpSearch.toLowerCase())
    || addMcpSearch.toLowerCase().includes('custom')
    || addMcpSearch.toLowerCase().includes('api');

  // Filter configs by search
  const searchLower = mcpSearch.toLowerCase();
  const filteredConfigsByServer = new Map<string, { serverId: string; serverName: string; configs: McpConfigDisplay[] }>();
  for (const [serverName, group] of configsByServer) {
    if (!mcpSearch) {
      filteredConfigsByServer.set(serverName, group);
    } else {
      const nameMatch = serverName.toLowerCase().includes(searchLower);
      const filteredConfigs = group.configs.filter(c =>
        nameMatch || c.label.toLowerCase().includes(searchLower) ||
        c.project_names.some(n => n.toLowerCase().includes(searchLower))
      );
      if (filteredConfigs.length > 0) {
        filteredConfigsByServer.set(serverName, { ...group, configs: filteredConfigs });
      }
    }
  }
  // ── Render ──

  return (
    <div>
      {/* 0.8.6 (#33 fix 2026-05-21) — Custom plugin export modal.
          Renders unconditionally at the top so it survives navigation
          inside McpPage (detail panel state can flip while the modal
          is open). Surfaces the JSON in a readonly textarea + best-
          effort clipboard write, with a copy-retry button when the
          auto-copy failed (Tauri / sandboxed webview case). */}
      {exportPayload && (
        <div
          className="mcp-export-modal-backdrop"
          data-testid="mcp-export-modal"
          onClick={closeExportModal}
        >
          <div
            className="mcp-export-modal"
            onClick={e => e.stopPropagation()}
          >
            <div className="mcp-export-modal-header">
              <span>
                <Upload size={13} style={{ marginRight: 6 }} />
                {t('mcp.custom.exportTitle', exportPayload.name)}
              </span>
              <button
                className="mcp-icon-btn"
                onClick={closeExportModal}
                aria-label="Close export modal"
                data-testid="mcp-export-modal-close"
              >
                <X size={14} />
              </button>
            </div>
            <p className="mcp-export-modal-hint">
              {exportCopyState === 'copied'
                ? t('mcp.custom.copied')
                : exportCopyState === 'failed'
                  ? t('mcp.custom.copyManualInstruction')
                  : t('mcp.custom.exportHint')}
            </p>
            <textarea
              ref={exportTextareaRef}
              className="input mcp-input-mono"
              data-testid="mcp-export-modal-textarea"
              value={exportPayload.json}
              readOnly
              rows={14}
              autoFocus
              onFocus={e => e.currentTarget.select()}
            />
            <div className="flex-row gap-3 mt-3">
              <button
                type="button"
                className="mcp-btn-action mcp-btn-action-primary"
                onClick={handleExportRetryCopy}
                data-testid="mcp-export-modal-copy"
              >
                <Upload size={12} /> {t('mcp.custom.copyAsJson')}
              </button>
              {/* 0.8.6 (#63) — Path B file download. Blob the JSON
                  locally and trigger a download — no server round-trip,
                  no Auth headers to plumb. Works inside Tauri too. */}
              <button
                type="button"
                className="mcp-btn-action"
                onClick={() => handleExportDownloadFile(exportPayload.name, exportPayload.json)}
                data-testid="mcp-export-modal-download"
              >
                <Download size={12} /> {t('mcp.custom.downloadAsFile')}
              </button>
              <button
                type="button"
                className="mcp-btn-action"
                onClick={closeExportModal}
              >
                {t('mcp.back')}
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="mcp-page-header">
        <div>
          <h1 className="mcp-h1"><MatrixText text={t('mcp.title')} /> <span className="mcp-subtitle"><MatrixText text={t('mcp.subtitle')} /></span></h1>
          <p className="mcp-meta">
            {totalConfigs} {totalConfigs > 1 ? t('mcp.configPlural') : t('mcp.config')} · {servers.length} {servers.length > 1 ? t('mcp.serverPlural') : t('mcp.server')} · {globalConfigs.length} {globalConfigs.length > 1 ? t('mcp.globalPlural') : t('mcp.global')}
          </p>
        </div>
        <div className="flex-row gap-4">
          <button className="mcp-btn-action mcp-btn-action-primary" data-tour-id="add-plugin-btn" onClick={() => { setShowAddMcp(true); setAddMcpSelected(null); setAddMcpSearch(''); }} title={t('mcp.addTitle')}>
            <Plus size={14} /> {t('mcp.add')}
          </button>
          <button className="mcp-btn-action" disabled={syncing} onClick={async () => { setSyncing(true); try { await mcpsApi.refresh(); refetchMcps(); } catch (e) { console.warn('Failed to sync MCPs:', e); } finally { setSyncing(false); } }} title={t('mcp.detect')}>
            <RefreshCw size={14} className={syncing ? 'spin' : ''} /> {syncing ? t('mcp.syncing') : t('mcp.detect')}
          </button>
        </div>
      </div>

      {/* Incomplete-config warning banner. Lists the MCPs whose env_keys
          are declared but values are missing/empty — those would fail
          handshake at agent boot and slow down every Kronn-spawned run.
          The scanner already SKIPS them in project-level config files;
          this banner tells the operator which plugins to fix. Click on
          a row to jump to the config detail. */}
      {(mcpOverview.incomplete_configs?.length ?? 0) > 0 && (
        <div className="mcp-warning-banner" data-testid="mcp-incomplete-banner">
          <div className="mcp-warning-banner-title">
            ⚠ {t('mcp.incomplete.title', mcpOverview.incomplete_configs!.length)}
          </div>
          <p className="mcp-warning-banner-hint">{t('mcp.incomplete.hint')}</p>
          <ul className="mcp-warning-banner-list">
            {mcpOverview.incomplete_configs!.map(ic => (
              <li key={ic.config_id}>
                <button
                  type="button"
                  className="mcp-warning-banner-item"
                  onClick={() => setSelectedConfigId(ic.config_id)}
                >
                  <strong>{ic.label}</strong>
                  <span className="mcp-warning-banner-server"> · {ic.server_name}</span>
                  <span className="mcp-warning-banner-reason"> — {ic.reason}</span>
                  {ic.missing_keys.length > 0 && (
                    <code className="mcp-warning-banner-keys">{ic.missing_keys.join(', ')}</code>
                  )}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* ── Add MCP from registry ── */}
      {showAddMcp && (
        <div ref={addMcpRef} className="mcp-card mcp-add-panel">
          <div className="mcp-add-header">
            <h3 className="mcp-add-title">
              {addMcpSelected ? t('mcp.configure', selectedDef?.name ?? addMcpLabel) : t('mcp.addTitle')}
            </h3>
            <button className="mcp-icon-btn" onClick={() => { setShowAddMcp(false); setAddMcpSelected(null); }} aria-label="Close">
              <X size={14} />
            </button>
          </div>

          {!addMcpSelected ? (
            <>
              {/* 0.8.6 phase 4 — top-level type filter (MCP / API / CLI).
                  Distinct from the per-tag category filter below : this
                  one narrows by TRANSPORT KIND so the user can isolate
                  "show me only CLI wrappers" (gitlab, fastly) without
                  scrolling through MCP servers. `all` is the default. */}
              <div
                className="mcp-kind-filter-row"
                role="radiogroup"
                aria-label={t('mcp.kindFilter.label')}
                data-testid="mcp-kind-filter"
                style={{
                  display: 'flex',
                  gap: 6,
                  marginBottom: 10,
                  flexWrap: 'wrap',
                }}
              >
                {(['all', 'mcp', 'api', 'cli'] as const).map(kind => {
                  const labelKey = `mcp.kindFilter.${kind}`;
                  const icons: Record<typeof kind, string> = { all: '✱', mcp: '🔌', api: '🌐', cli: '⌨' };
                  const active = addMcpKindFilter === kind;
                  return (
                    <button
                      key={kind}
                      type="button"
                      role="radio"
                      aria-checked={active}
                      className="mcp-kind-filter-btn"
                      data-active={active}
                      data-testid={`mcp-kind-filter-${kind}`}
                      onClick={() => setAddMcpKindFilter(kind)}
                      style={{
                        padding: '4px 10px',
                        borderRadius: 6,
                        border: active
                          ? '1px solid var(--kr-accent, #c8a0ff)'
                          : '1px solid var(--kr-border-subtle, rgba(255,255,255,0.1))',
                        background: active
                          ? 'var(--kr-bg-accent-subtle, rgba(200,160,255,0.15))'
                          : 'transparent',
                        color: active ? 'var(--kr-text-primary)' : 'var(--kr-text-secondary)',
                        cursor: 'pointer',
                        fontSize: 12,
                      }}
                    >
                      <span style={{ marginRight: 4 }}>{icons[kind]}</span>
                      {t(labelKey)}
                    </button>
                  );
                })}
              </div>
              <input
                className="input mb-5"
                placeholder={t('mcp.searchRegistry')}
                value={addMcpSearch}
                onChange={(e) => setAddMcpSearch(e.target.value)}
                autoFocus
              />
              {/* Category filter pills */}
              {(() => {
                const categoryMap: Record<string, string> = {
                  // 0.8.6 phase 4 — `cli` first so plugins that WRAP a
                  // local CLI binary (Fastly, GitLab via glab, …) land in
                  // their own bucket rather than the generic Git/Code or
                  // Cloud groups. Same prereq surface as a CLI agent —
                  // the user must install the binary on the host first.
                  cli: t('mcp.cat.cli'),
                  git: t('mcp.cat.gitCode'), code: t('mcp.cat.gitCode'),
                  database: t('mcp.cat.databases'), sql: t('mcp.cat.databases'), cache: t('mcp.cat.databases'), embedded: t('mcp.cat.databases'),
                  cloud: t('mcp.cat.cloud'), containers: t('mcp.cat.cloud'), devops: t('mcp.cat.cloud'),
                  search: t('mcp.cat.search'), web: t('mcp.cat.search'), http: t('mcp.cat.search'), browser: t('mcp.cat.search'), scraping: t('mcp.cat.search'),
                  monitoring: t('mcp.cat.monitoring'), analytics: t('mcp.cat.monitoring'), errors: t('mcp.cat.monitoring'),
                  communication: t('mcp.cat.communication'), chat: t('mcp.cat.communication'), email: t('mcp.cat.communication'), mailing: t('mcp.cat.communication'),
                  'project-management': t('mcp.cat.projectMgmt'), issues: t('mcp.cat.projectMgmt'),
                  core: t('mcp.cat.utilities'), filesystem: t('mcp.cat.utilities'), docs: t('mcp.cat.utilities'), libraries: t('mcp.cat.utilities'),
                  design: t('mcp.cat.design'),
                };
                const getCategory = (tags: string[]) => {
                  for (const tag of tags) { if (categoryMap[tag]) return categoryMap[tag]; }
                  return t('mcp.cat.other');
                };
                const categoryOrder = [t('mcp.cat.cli'), t('mcp.cat.gitCode'), t('mcp.cat.databases'), t('mcp.cat.cloud'), t('mcp.cat.search'), t('mcp.cat.monitoring'), t('mcp.cat.communication'), t('mcp.cat.projectMgmt'), t('mcp.cat.design'), t('mcp.cat.utilities'), t('mcp.cat.other')];
                const grouped = new Map<string, typeof availableRegistry>();
                for (const m of availableRegistry) {
                  const cat = getCategory(m.tags);
                  let bucket = grouped.get(cat);
                  if (!bucket) {
                    bucket = [];
                    grouped.set(cat, bucket);
                  }
                  bucket.push(m);
                }
                const catsWithItems = categoryOrder.filter(cat => grouped.has(cat));
                return (
                  <>
                    <div className="mcp-cat-pills">
                      <button
                        className={`mcp-cat-pill${!selectedCategory ? ' mcp-cat-pill-active' : ''}`}
                        onClick={() => setSelectedCategory(null)}
                      >
                        {t('mcp.cat.all')}
                      </button>
                      {catsWithItems.map(cat => (
                        <button
                          key={cat}
                          className={`mcp-cat-pill${selectedCategory === cat ? ' mcp-cat-pill-active' : ''}`}
                          onClick={() => setSelectedCategory(selectedCategory === cat ? null : cat)}
                        >
                          {cat} <span className="mcp-cat-pill-count">{grouped.get(cat)?.length ?? 0}</span>
                        </button>
                      ))}
                    </div>
                    <div className="mcp-registry-grid">
                      {customApiVisible && (addMcpKindFilter === 'all' || addMcpKindFilter === 'api') && !selectedCategory ? (
                        <div
                          key="api-import"
                          className="mcp-registry-card mcp-registry-card-custom"
                          onClick={() => {
                            setAddMcpSelected('api-import');
                            setAddMcpLabel('');
                          }}
                          data-testid="mcp-import-json-tile"
                        >
                          <div className="mcp-registry-card-top">
                            <div className="mcp-registry-card-icon">
                              <Download size={16} />
                            </div>
                            <div className="flex-1">
                              <div className="mcp-registry-card-name">{t('mcp.custom.importTileTitle')}</div>
                              <div className="mcp-registry-card-cat">{t('mcp.custom.tileCat')}</div>
                            </div>
                          </div>
                          <div className="mcp-registry-card-desc">{t('mcp.custom.importTileDesc')}</div>
                          <div className="mcp-registry-card-meta">
                            <span className="mcp-kind-badge mcp-kind-badge-api" title={t('mcp.kind.apiTooltip')}>
                              <Globe size={9} /> {t('mcp.kind.api')}
                            </span>
                            <span className="mcp-origin-badge mcp-origin-community">
                              {t('mcp.custom.tileBadge')}
                            </span>
                          </div>
                        </div>
                      ) : null}
                      {customApiVisible && (addMcpKindFilter === 'all' || addMcpKindFilter === 'api') && !selectedCategory ? (
                        <div
                          key="custom-api"
                          className="mcp-registry-card mcp-registry-card-custom"
                          onClick={() => {
                            setAddMcpSelected('api-custom');
                            setAddMcpLabel('');
                          }}
                          data-tour-id="custom-api-tile"
                        >
                          <div className="mcp-registry-card-top">
                            <div className="mcp-registry-card-icon">
                              <Plus size={16} />
                            </div>
                            <div className="flex-1">
                              <div className="mcp-registry-card-name">{t('mcp.custom.tileTitle')}</div>
                              <div className="mcp-registry-card-cat">{t('mcp.custom.tileCat')}</div>
                            </div>
                          </div>
                          <div className="mcp-registry-card-desc">{t('mcp.custom.tileDesc')}</div>
                          <div className="mcp-registry-card-meta">
                            <span className="mcp-kind-badge mcp-kind-badge-api" title={t('mcp.kind.apiTooltip')}>
                              <Globe size={9} /> {t('mcp.kind.api')}
                            </span>
                            <span className="mcp-origin-badge mcp-origin-community">
                              {t('mcp.custom.tileBadge')}
                            </span>
                          </div>
                        </div>
                      ) : null}
                      {catsWithItems.flatMap(cat =>
                        (grouped.get(cat) ?? [])
                          .filter(m => {
                            // Category filter (kind filtering already applied
                            // upstream via `availableRegistry` / addMcpKindFilter).
                            if (selectedCategory && selectedCategory !== cat) return false;
                            // Text search filter
                            if (addMcpSearch && !m.name.toLowerCase().includes(addMcpSearch.toLowerCase()) && !m.tags.some(tag => tag.toLowerCase().includes(addMcpSearch.toLowerCase()))) return false;
                            return true;
                          })
                          .map(m => {
                            const alreadyAdded = configuredServerIds.has(m.id);
                            return (
                              <div
                                key={m.id}
                                className={`mcp-registry-card${alreadyAdded ? ' mcp-registry-card-installed' : ''}`}
                                onClick={() => {
                                  setAddMcpSelected(m.id);
                                  setAddMcpLabel(alreadyAdded ? `${m.name} (${configs.filter(c => c.server_name === m.name).length + 1})` : m.name);
                                  const envInit: Record<string, string> = {};
                                  m.env_keys.forEach(k => { envInit[k] = ''; });
                                  setAddMcpEnv(envInit);
                                }}
                              >
                                <div className="mcp-registry-card-top">
                                  <div className="mcp-registry-card-icon">
                                    <Puzzle size={16} />
                                  </div>
                                  <div className="flex-1">
                                    <div className="mcp-registry-card-name">{m.name}</div>
                                    <div className="mcp-registry-card-cat">{getCategory(m.tags)}</div>
                                  </div>
                                  {alreadyAdded && <Check size={14} className="text-info" />}
                                </div>
                                <div className="mcp-registry-card-desc">{m.description}</div>
                                <div className="mcp-registry-card-meta">
                                  {(() => {
                                    const kind = pluginKind(m);
                                    const label = kind === 'api'
                                      ? t('mcp.kind.api')
                                      : kind === 'hybrid'
                                        ? t('mcp.kind.hybrid')
                                        : t('mcp.kind.mcp');
                                    const Icon = kind === 'mcp' ? Plug : kind === 'api' ? Globe : Puzzle;
                                    return (
                                      <span className={`mcp-kind-badge mcp-kind-badge-${kind}`} title={t(`mcp.kind.${kind}Tooltip`)}>
                                        <Icon size={9} /> {label}
                                      </span>
                                    );
                                  })()}
                                  <span className={`mcp-origin-badge ${m.official ? 'mcp-origin-official' : 'mcp-origin-community'}`}>
                                    {m.official ? t('mcp.official') : t('mcp.community')} — {m.publisher}
                                  </span>
                                  {(m.env_keys.length > 0 || m.token_help) && (
                                    <span><Key size={9} /> {t('mcp.setupRequired')}</span>
                                  )}
                                </div>
                              </div>
                            );
                          })
                      )}
                    </div>
                  </>
                );
              })()}
            </>
          ) : addMcpSelected === 'api-import' ? (
            <>
              {/* 0.8.6 (#33) — Import-from-JSON form. Paste the export
                  payload from another Kronn install, validate light
                  (name + base_url required), then POST. Credentials are
                  NEVER imported even if the JSON contains values. */}
              <div className="mb-5" data-testid="mcp-import-json-form">
                <label className="mcp-field-label">{t('mcp.custom.importPasteLabel')} *</label>
                <textarea
                  className="input mcp-input-mono"
                  rows={12}
                  value={importJsonText}
                  onChange={e => { setImportJsonText(e.target.value); if (importJsonError) setImportJsonError(null); }}
                  placeholder={t('mcp.custom.importPlaceholder')}
                  autoFocus
                  data-testid="mcp-import-json-textarea"
                />
              </div>
              <div className="flex-row gap-3 mb-5">
                <button
                  type="button"
                  className="mcp-btn-action"
                  onClick={handlePasteImportJson}
                  data-testid="mcp-import-paste-clipboard"
                >
                  <Download size={12} /> {t('mcp.custom.importPasteFromClipboard')}
                </button>
                {/* 0.8.6 (#63) — Path B file upload. Hidden input
                    triggered by a styled button so the UX matches the
                    other action buttons. */}
                <label className="mcp-btn-action" style={{ cursor: 'pointer', display: 'inline-flex' }}>
                  <Upload size={12} /> {t('mcp.custom.importFromFile')}
                  <input
                    type="file"
                    accept=".json,application/json"
                    style={{ display: 'none' }}
                    onChange={e => {
                      const f = e.target.files?.[0];
                      if (f) handleImportFromFile(f);
                      // Reset so re-picking the same file fires onChange again.
                      e.target.value = '';
                    }}
                    data-testid="mcp-import-file-input"
                  />
                </label>
                <button
                  type="button"
                  className="mcp-btn-action mcp-btn-action-primary"
                  onClick={handleImportCustomPlugin}
                  disabled={importJsonLoading || !importJsonText.trim()}
                  data-testid="mcp-import-submit"
                >
                  <Plus size={12} /> {t('mcp.custom.importSubmit')}
                </button>
              </div>
              {importJsonError && (
                <div className="mcp-form-error" data-testid="mcp-import-error">
                  {importJsonError}
                </div>
              )}
              <p className="mcp-form-hint">{t('mcp.custom.importSecretsHint')}</p>
            </>
          ) : addMcpSelected === 'api-custom' ? (
            <>
              {/* Custom API freeform editor. Mirrors `CustomApiPayload`:
                  name + base URL are required; description + docs link +
                  fields are optional. The backend slugifies each field
                  label into an env key. */}
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.name')} *</label>
                <input
                  className="input"
                  value={customName}
                  onChange={(e) => setCustomName(e.target.value)}
                  placeholder={t('mcp.custom.namePlaceholder')}
                  autoFocus
                />
              </div>
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.baseUrl')} *</label>
                <input
                  className="input mcp-input-mono"
                  value={customBaseUrl}
                  onChange={(e) => setCustomBaseUrl(e.target.value)}
                  placeholder={t('mcp.custom.baseUrlPlaceholder')}
                />
              </div>
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.description')}</label>
                <textarea
                  className="input"
                  rows={3}
                  value={customDescription}
                  onChange={(e) => setCustomDescription(e.target.value)}
                  placeholder={t('mcp.custom.descriptionPlaceholder')}
                />
              </div>
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.docsUrl')}</label>
                <input
                  className="input mcp-input-mono"
                  value={customDocsUrl}
                  onChange={(e) => setCustomDocsUrl(e.target.value)}
                  placeholder="https://docs.example.com/api"
                />
              </div>
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.fields')}</label>
                <p className="mcp-env-key-desc mb-3">{t('mcp.custom.fieldsHint')}</p>
                {/* 0.8.6 — Edit mode: reassure the user their secrets
                    are safe. The encrypted env lives in the per-config
                    row (mcp_configs), NOT in the spec we're editing.
                    Without this banner, users see empty value fields
                    and assume their credentials were wiped (caught
                    2026-05-19 in live test "je n'ai plus les valeurs").
                    The masked `•••• stocké` per-row makes the same
                    point inline. */}
                {editingCustomServerId && (
                  <p className="mcp-env-key-desc mb-3" style={{ borderLeft: '3px solid var(--kr-accent)', paddingLeft: '0.6rem' }}>
                    🔒 {t('mcp.custom.fieldsHintEditMode')}
                  </p>
                )}
                {customFields.map((f, idx) => {
                  // 0.8.6 — per-row reveal toggle. The visible set is
                  // keyed by row index because labels can be renamed
                  // mid-edit (slug-based keys would race the rename).
                  const isVisible = customFieldsVisible.has(idx);
                  return (
                  <div key={idx} className="mcp-custom-field-row mb-2">
                    <input
                      className="input mcp-custom-field-label"
                      value={f.label}
                      onChange={(e) => setCustomFields(prev => prev.map((row, i) => i === idx ? { ...row, label: e.target.value } : row))}
                      placeholder={t('mcp.custom.fieldLabel')}
                    />
                    <input
                      className="input mcp-input-mono"
                      type={isVisible ? 'text' : 'password'}
                      value={f.value}
                      onChange={(e) => setCustomFields(prev => prev.map((row, i) => i === idx ? { ...row, value: e.target.value } : row))}
                      placeholder={t('mcp.custom.fieldValue')}
                    />
                    <button
                      type="button"
                      className="mcp-icon-btn"
                      onClick={() => setCustomFieldsVisible(prev => {
                        const next = new Set(prev);
                        if (next.has(idx)) next.delete(idx); else next.add(idx);
                        return next;
                      })}
                      title={isVisible ? t('mcp.hide') : t('mcp.show')}
                      aria-label={isVisible ? t('mcp.hide') : t('mcp.show')}
                    >
                      <Eye size={12} style={{ color: isVisible ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} />
                    </button>
                    <button
                      type="button"
                      className="mcp-icon-btn"
                      onClick={() => setCustomFields(prev => prev.filter((_, i) => i !== idx))}
                      disabled={customFields.length === 1}
                      aria-label={t('mcp.custom.fieldRemove')}
                      title={t('mcp.custom.fieldRemove')}
                    >
                      <X size={12} />
                    </button>
                  </div>
                  );
                })}
                <button
                  type="button"
                  className="mcp-btn-action"
                  onClick={() => setCustomFields(prev => [...prev, { label: '', value: '' }])}
                >
                  <Plus size={12} /> {t('mcp.custom.fieldAdd')}
                </button>
              </div>
              {/* 0.8.6 — endpoints declared at creation time. The AI helper
                  (button further below) can populate this list after fetching
                  `docs_url` via WebFetch. Empty list = `mcp_list` will emit
                  `NEEDS_RESEARCH` and agents will go through the doc each time.
                  Cf. [[project_endpoints_autodiscovery_0_8_6]]. */}
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.endpoints.header')}</label>
                <p className="mcp-env-key-desc mb-3">
                  {customEndpoints.length === 0
                    ? t('mcp.custom.endpoints.emptyHint')
                    : t('mcp.custom.endpoints.populatedHint', customEndpoints.length)}
                </p>
                {customEndpoints.map((e, idx) => (
                  <div key={idx} className="mcp-custom-field-row mb-2">
                    <select
                      className="input mcp-custom-field-label"
                      value={e.method || 'GET'}
                      onChange={(ev) => setCustomEndpoints(prev => prev.map((row, i) => i === idx ? { ...row, method: ev.target.value } : row))}
                      aria-label={t('mcp.custom.endpoints.methodLabel')}
                    >
                      <option value="GET">GET</option>
                      <option value="POST">POST</option>
                      <option value="PUT">PUT</option>
                      <option value="PATCH">PATCH</option>
                      <option value="DELETE">DELETE</option>
                    </select>
                    <input
                      className="input mcp-input-mono"
                      value={e.path}
                      onChange={(ev) => setCustomEndpoints(prev => prev.map((row, i) => i === idx ? { ...row, path: ev.target.value } : row))}
                      placeholder="/v1/widgets"
                    />
                    <input
                      className="input"
                      value={e.description}
                      onChange={(ev) => setCustomEndpoints(prev => prev.map((row, i) => i === idx ? { ...row, description: ev.target.value } : row))}
                      placeholder={t('mcp.custom.endpoints.descPlaceholder')}
                    />
                    <button
                      type="button"
                      className="mcp-icon-btn"
                      onClick={() => setCustomEndpoints(prev => prev.filter((_, i) => i !== idx))}
                      aria-label={t('mcp.custom.endpoints.remove')}
                      title={t('mcp.custom.endpoints.remove')}
                    >
                      <X size={12} />
                    </button>
                  </div>
                ))}
                <button
                  type="button"
                  className="mcp-btn-action"
                  onClick={() => setCustomEndpoints(prev => [...prev, { path: '', method: 'GET', description: '' }])}
                >
                  <Plus size={12} /> {t('mcp.custom.endpoints.add')}
                </button>
              </div>
              {/* 0.8.6 — Auth section. MVP exposes 3 of the 7 runtime
                  variants (None, Bearer, TokenExchange) — others (Header,
                  Query, Basic, OAuth2) come in 0.8.6 Layer A. The
                  TokenExchange option specifically unblocks Didomi-shape
                  APIs (POST /sessions with JSON body → access_token →
                  Bearer). Cf. [[project_token_exchange_generic_0_9_0]]. */}
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.custom.auth.header')}</label>
                <p className="mcp-env-key-desc mb-3">{t('mcp.custom.auth.hint')}</p>
                <select
                  className="input mcp-custom-field-label"
                  value={authKindOf(customAuth)}
                  onChange={(e) => setAuthKindBy(e.target.value as 'None' | 'Bearer' | 'TokenExchange')}
                  style={{ marginBottom: '0.75rem', width: '100%' }}
                >
                  <option value="None">{t('mcp.custom.auth.kind.none')}</option>
                  <option value="Bearer">{t('mcp.custom.auth.kind.bearer')}</option>
                  <option value="TokenExchange">{t('mcp.custom.auth.kind.tokenExchange')}</option>
                  {authKindOf(customAuth) === 'Other' && (
                    <option value="Other" disabled>{t('mcp.custom.auth.kind.other')}</option>
                  )}
                </select>
                {/* Bearer — 1 env_key dropdown peuplé depuis customFields */}
                {authKindOf(customAuth) === 'Bearer' && typeof customAuth === 'object' && 'Bearer' in customAuth && (
                  <div className="mcp-custom-field-row mb-2">
                    <label className="mcp-field-label mcp-field-label-inline" style={{ minWidth: '120px' }}>
                      {t('mcp.custom.auth.bearer.envKey')}
                    </label>
                    <select
                      className="input mcp-input-mono"
                      value={customAuth.Bearer.env_key}
                      onChange={(e) => setCustomAuth({ Bearer: { env_key: e.target.value } })}
                    >
                      <option value="">{t('mcp.custom.auth.bearer.envKeyPlaceholder')}</option>
                      {customFields.filter(f => f.label.trim()).map(f => (
                        <option key={f.label} value={slugEnvKey(f.label)}>
                          {slugEnvKey(f.label)} ({f.label})
                        </option>
                      ))}
                    </select>
                  </div>
                )}
                {/* TokenExchange — Didomi pattern, JSON body POST → access_token */}
                {authKindOf(customAuth) === 'TokenExchange' && typeof customAuth === 'object' && 'TokenExchange' in customAuth && (
                  <div style={{ borderLeft: '3px solid var(--kr-accent)', paddingLeft: '0.8rem' }}>
                    <p className="mcp-env-key-desc mb-3" style={{ fontSize: '0.85em' }}>{t('mcp.custom.auth.tokenExchange.hint')}</p>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.endpoint')}</label>
                      <input
                        className="input mcp-input-mono"
                        value={customAuth.TokenExchange.endpoint}
                        onChange={(e) => setCustomAuth({
                          TokenExchange: { ...customAuth.TokenExchange, endpoint: e.target.value },
                        })}
                        placeholder="/sessions"
                      />
                    </div>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.method')}</label>
                      <select
                        className="input mcp-input-mono"
                        value={customAuth.TokenExchange.method}
                        onChange={(e) => setCustomAuth({
                          TokenExchange: { ...customAuth.TokenExchange, method: e.target.value },
                        })}
                      >
                        <option value="POST">POST</option>
                        <option value="PUT">PUT</option>
                      </select>
                    </div>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.bodyFormat')}</label>
                      <select
                        className="input mcp-input-mono"
                        value={customAuth.TokenExchange.body_format}
                        onChange={(e) => setCustomAuth({
                          TokenExchange: { ...customAuth.TokenExchange, body_format: e.target.value as 'Json' | 'FormUrlEncoded' },
                        })}
                      >
                        <option value="Json">JSON (application/json)</option>
                        <option value="FormUrlEncoded">Form URL-encoded (application/x-www-form-urlencoded)</option>
                      </select>
                    </div>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.bodyTemplate')}</label>
                      <p className="mcp-env-key-desc mb-2" style={{ fontSize: '0.8em' }}>{t('mcp.custom.auth.tokenExchange.bodyTemplateHint')}</p>
                      <textarea
                        className="input mcp-input-mono"
                        rows={5}
                        value={(() => {
                          try { return JSON.stringify(customAuth.TokenExchange.body_template, null, 2); }
                          catch { return '{}'; }
                        })()}
                        onChange={(e) => {
                          try {
                            const parsed = JSON.parse(e.target.value);
                            setCustomAuth({ TokenExchange: { ...customAuth.TokenExchange, body_template: parsed } });
                          } catch {
                            // Keep the textarea contents user-typed even when invalid;
                            // we store the raw text via a sibling state? Simpler: just
                            // don't update the parsed value on invalid JSON. The user
                            // sees their typo and corrects.
                          }
                        }}
                        placeholder='{"type": "api-key", "key": "${ENV.API_KEY}", "secret": "${ENV.API_SECRET}"}'
                      />
                    </div>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.tokenJsonpath')}</label>
                      <input
                        className="input mcp-input-mono"
                        value={customAuth.TokenExchange.token_jsonpath}
                        onChange={(e) => setCustomAuth({
                          TokenExchange: { ...customAuth.TokenExchange, token_jsonpath: e.target.value },
                        })}
                        placeholder="$.access_token"
                      />
                    </div>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.ttl')}</label>
                      <input
                        className="input mcp-input-mono"
                        type="number"
                        min={0}
                        value={customAuth.TokenExchange.ttl_seconds}
                        onChange={(e) => setCustomAuth({
                          TokenExchange: { ...customAuth.TokenExchange, ttl_seconds: parseInt(e.target.value, 10) || 0 },
                        })}
                      />
                    </div>
                    <div className="mb-3">
                      <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.inject')}</label>
                      <Dropdown<'BearerHeader' | 'CustomHeader' | 'QueryParam'>
                        value={typeof customAuth.TokenExchange.inject === 'string' ? customAuth.TokenExchange.inject : (Object.keys(customAuth.TokenExchange.inject)[0] as 'CustomHeader' | 'QueryParam')}
                        options={[
                          { value: 'BearerHeader', label: 'Bearer header (Authorization: Bearer ...)' },
                          { value: 'CustomHeader', label: 'Custom header' },
                          { value: 'QueryParam', label: 'Query param' },
                        ]}
                        onChange={(kind) => {
                          let inject: ApiAuthKind extends infer T ? T extends { TokenExchange: { inject: infer I } } ? I : never : never;
                          if (kind === 'BearerHeader') inject = 'BearerHeader' as typeof inject;
                          else if (kind === 'CustomHeader') inject = { CustomHeader: { name: 'X-Auth-Token' } } as typeof inject;
                          else inject = { QueryParam: { name: 'token' } } as typeof inject;
                          setCustomAuth({ TokenExchange: { ...customAuth.TokenExchange, inject } });
                        }}
                        ariaLabel={t('mcp.custom.auth.tokenExchange.inject')}
                        testId="mcp-token-exchange-inject"
                      />
                    </div>
                  </div>
                )}
                {/* Other variants (ApiKeyQuery / Header / Basic / OAuth2) :
                    not yet exposed in UI. If an existing plugin uses one of
                    them (e.g. registry-shipped), the picker shows "Other —
                    edit in JSON" placeholder so the user knows it's
                    intentional, not a bug. */}
                {authKindOf(customAuth) === 'Other' && (
                  <p className="mcp-env-key-desc" style={{ color: 'var(--kr-warning)' }}>
                    {t('mcp.custom.auth.kind.otherWarning')}
                  </p>
                )}
              </div>
              <div className="flex-row gap-4 mb-6">
                <button className={`mcp-project-toggle ${addMcpGlobal ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`} onClick={() => setAddMcpGlobal(!addMcpGlobal)}>
                  {addMcpGlobal ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                  {t('mcp.globalAll')}
                </button>
              </div>
              <div className="flex-row gap-4">
                <button
                  className="mcp-btn-action mcp-btn-action-primary"
                  onClick={handleAddMcpFromRegistry}
                  disabled={!customName.trim() || !customBaseUrl.trim()}
                >
                  <Check size={14} /> {editingCustomServerId ? t('mcp.custom.saveEdit') : t('mcp.custom.save')}
                </button>
                <button className="mcp-btn-action" onClick={() => { setAddMcpSelected(null); resetAddMcp(); }}>
                  {t('mcp.back')}
                </button>
                {/* AI helper bubble: pre-fills the form from a curl, a docs link
                    or a freeform description. Same UX as the workflow ApiCall
                    helper (header agent dropdown, top context chip, welcome
                    starters). */}
                {installedAgentTypes && installedAgentTypes.length > 0 && (
                  <CustomApiAiHelper
                    formSnapshot={{
                      name: customName,
                      base_url: customBaseUrl,
                      description: customDescription,
                      docs_url: customDocsUrl,
                      fields: customFields,
                      endpoints: customEndpoints,
                    }}
                    onApply={(updates: Partial<CustomApiPayload>) => {
                      if (typeof updates.name === 'string') setCustomName(updates.name);
                      if (typeof updates.base_url === 'string') setCustomBaseUrl(updates.base_url);
                      if (typeof updates.description === 'string') setCustomDescription(updates.description);
                      if (typeof updates.docs_url === 'string') setCustomDocsUrl(updates.docs_url);
                      if (Array.isArray(updates.fields) && updates.fields.length > 0) {
                        // Merge: keep user-typed values for fields that already
                        // have content, accept agent-proposed labels/empties for
                        // the rest. Avoids the agent wiping a token the user
                        // already pasted while still letting it add new fields.
                        const existing = customFields.filter(f => f.label.trim() || f.value.trim());
                        const proposedLabels = new Set(updates.fields.map(f => f.label));
                        const merged = [
                          ...existing.filter(f => !proposedLabels.has(f.label) || f.value.trim()),
                          ...updates.fields.filter(f =>
                            !existing.some(e => e.label === f.label && e.value.trim()),
                          ),
                        ];
                        setCustomFields(merged.length > 0 ? merged : [{ label: '', value: '' }]);
                      }
                      // 0.8.6 — endpoint merge. The agent typically proposes
                      // 5-15 endpoints after a WebFetch. We merge by
                      // (path + method) so the user's hand-typed entries are
                      // preserved (no surprise wipe), and the agent's proposals
                      // fill the gaps. The "Add row" trailing-empty sentinel is
                      // filtered out of the seed.
                      if (Array.isArray(updates.endpoints) && updates.endpoints.length > 0) {
                        const existing = customEndpoints.filter(e => e.path.trim() !== '');
                        const key = (e: ApiEndpoint) => `${e.method.toUpperCase()} ${e.path.trim()}`;
                        const seen = new Set(existing.map(key));
                        const merged: ApiEndpoint[] = [
                          ...existing,
                          ...updates.endpoints.filter(e => !seen.has(key(e))),
                        ];
                        setCustomEndpoints(merged);
                      }
                    }}
                    installedAgents={installedAgentTypes}
                    configLanguage={configLanguage}
                    t={t}
                  />
                )}
              </div>
            </>
          ) : (
            <>
              {/* Label */}
              <div className="mb-5">
                <label className="mcp-field-label">{t('mcp.label')}</label>
                <input
                  className="input"
                  value={addMcpLabel}
                  onChange={(e) => setAddMcpLabel(e.target.value)}
                  placeholder={selectedDef?.name ?? 'Label'}
                />
              </div>
              {/* Env vars */}
              {(() => {
                const envKeys = selectedDef?.env_keys ?? mcpOverview.configs.find(c => c.server_id === addMcpSelected)?.env_keys ?? [];
                return envKeys.length > 0 ? (
                <div className="mb-5">
                  <div className="flex-row gap-4 mb-3">
                    <label className="mcp-field-label mcp-field-label-inline">{t('mcp.envVars')}</label>
                    {selectedDef?.token_url && (
                      <a
                        href={selectedDef.token_url}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="mcp-token-link"
                      >
                        <ExternalLink size={10} />
                        {selectedDef.token_help ?? t('mcp.getToken')}
                      </a>
                    )}
                    {!selectedDef?.token_url && selectedDef?.token_help && (
                      <span className="mcp-token-hint">{selectedDef.token_help}</span>
                    )}
                  </div>
                  {envKeys.map(k => {
                    const isVisible = addVisibleFields.has(k);
                    // Prefer per-plugin metadata (api_spec.config_keys) over
                    // the global ENV_PLACEHOLDERS map — that way any future
                    // API plugin gets meaningful placeholders via its own
                    // registry entry, no code change needed here.
                    const configKey = selectedDef?.api_spec?.config_keys?.find(c => c.env_key === k);
                    const hint = configKey?.placeholder
                      ?? ENV_PLACEHOLDERS[k]
                      ?? ENV_PLACEHOLDERS[k.replace(/^.*_/, '')] // fallback: match suffix (e.g. _API_KEY → API_KEY)
                      ?? t('mcp.value');
                    // Non-secret config keys are rendered as plain text
                    // (no masking) — they're not credentials and hiding
                    // them behind dots just makes the form unusable.
                    const isPlainTextConfig = !!configKey;
                    return (
                      <div key={k} className="mb-2">
                        <div className="flex-row gap-4">
                          <span className="mcp-env-key-label">{configKey?.label ?? k}</span>
                          <div className="mcp-env-input-wrap">
                            <input
                              className="input mcp-input-mono mcp-input-with-eye"
                              value={addMcpEnv[k] ?? ''}
                              onChange={(e) => setAddMcpEnv(prev => ({ ...prev, [k]: e.target.value }))}
                              placeholder={hint}
                              type={isPlainTextConfig || isVisible ? 'text' : 'password'}
                            />
                            {!isPlainTextConfig && (
                              <button
                                type="button"
                                className="mcp-eye-btn"
                                onClick={() => setAddVisibleFields(prev => {
                                  const next = new Set(prev);
                                  if (next.has(k)) next.delete(k); else next.add(k);
                                  return next;
                                })}
                                tabIndex={-1}
                              >
                                <Eye size={12} style={{ color: isVisible ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} />
                              </button>
                            )}
                          </div>
                        </div>
                        {/* Inline description from api_spec.config_keys
                            (e.g. Chartbeat host explains "the site tracked
                            in Chartbeat"). Static ENV_PLACEHOLDERS map has
                            no equivalent, so this only fires for API
                            plugins. */}
                        {configKey?.description && (
                          <div className="mcp-env-key-desc">{configKey.description}</div>
                        )}
                      </div>
                    );
                  })}
                </div>
              ) : null; })()}
              {/* Global toggle */}
              <div className="flex-row gap-4 mb-6">
                <button className={`mcp-project-toggle ${addMcpGlobal ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`} onClick={() => setAddMcpGlobal(!addMcpGlobal)}>
                  {addMcpGlobal ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                  {t('mcp.globalAll')}
                </button>
              </div>
              {/* Actions */}
              <div className="flex-row gap-4">
                <button
                  className="mcp-btn-action mcp-btn-action-primary"
                  onClick={handleAddMcpFromRegistry}
                >
                  <Check size={14} /> {t('mcp.addBtn')}
                </button>
                <button className="mcp-btn-action" onClick={() => setAddMcpSelected(null)}>
                  {t('mcp.back')}
                </button>
              </div>
            </>
          )}
        </div>
      )}

      {/* ── Search bar ── */}
      {totalConfigs > 3 && (
        <div className="mcp-search-wrap">
          <Search size={14} className="mcp-search-icon" />
          <input
            className="input mcp-search-input"
            placeholder={t('mcp.search')}
            value={mcpSearch}
            onChange={(e) => setMcpSearch(e.target.value)}
          />
          {mcpSearch && (
            <button
              className="mcp-search-clear"
              onClick={() => setMcpSearch('')}
              aria-label="Clear search"
            >
              <X size={12} />
            </button>
          )}
          <button
            className="mcp-btn-action mcp-sort-toggle"
            onClick={() => setMcpSort(mcpSort === 'az' ? 'za' : 'az')}
            title={mcpSort === 'az' ? t('mcp.sortAz') : t('mcp.sortZa')}
            aria-label={mcpSort === 'az' ? t('mcp.sortAz') : t('mcp.sortZa')}
          >
            {mcpSort === 'az' ? <ArrowDownAZ size={14} /> : <ArrowDownZA size={14} />}
          </button>
        </div>
      )}

      {/* ── Empty-state banner: no MCP exposed in CLI hors Kronn (UX#7) ── */}
      <CliExposureHint configs={configs} onJumpToConfig={(id) => setSelectedConfigId(id)} />

      {/* ── Installed plugins grid (detail expands inline) ── */}
      {totalConfigs > 0 ? (
        <div className="mcp-installed-grid">
          {[...configs]
            .sort((a, b) => {
              const cmp = a.label.localeCompare(b.label, undefined, { sensitivity: 'base' });
              return mcpSort === 'az' ? cmp : -cmp;
            })
            .filter(cfg => {
              if (!mcpSearch) return true;
              const s = mcpSearch.toLowerCase();
              return cfg.label.toLowerCase().includes(s) || cfg.server_name.toLowerCase().includes(s) || cfg.project_names.some(n => n.toLowerCase().includes(s));
            })
            .flatMap(cfg => {
              const linkedProjects = cfg.is_global ? projects.filter(p => !isHiddenPath(p.path)).length : cfg.project_ids.length;
              const isSelected = selectedConfigId === cfg.id;
              // 0.7.0 — derive plugin kind from the server registry so
              // we can hide host-sync UI on API-only plugins (they're
              // injected into prompts, never written to ~/.claude.json
              // & co — showing a "Sync CLI" toggle on them was a UX bug).
              const cfgServer = mcpOverview.servers.find(s => s.id === cfg.server_id);
              const cfgKind: PluginKind = cfgServer ? pluginKind(cfgServer) : 'mcp';
              const supportsHostSync = cfgKind !== 'api';

              const card = (
                <div
                  key={cfg.id}
                  className={`mcp-installed-card${isSelected ? ' mcp-installed-card-selected' : ''}`}
                  onClick={() => setSelectedConfigId(isSelected ? null : cfg.id)}
                >
                  <div className="mcp-installed-top">
                    <div className="mcp-registry-card-icon"><Puzzle size={16} /></div>
                    <div className="flex-1" style={{ minWidth: 0 }}>
                      <div className="mcp-installed-name">{cfg.label}</div>
                      <div className="mcp-installed-scope">
                        {cfg.is_global
                          ? <span className="mcp-scope-badge mcp-scope-global">Global</span>
                          : linkedProjects > 0
                            ? <span className="mcp-scope-badge mcp-scope-projects">{linkedProjects} {linkedProjects > 1 ? t('mcp.projectPlural') : t('mcp.project')}</span>
                            : <span className="mcp-scope-badge mcp-scope-none">{t('wiz.noProject')}</span>
                        }
                        {cfg.env_keys.length > 0 && <span className="mcp-installed-keys"><Key size={9} /> {cfg.env_keys.length}</span>}
                        {cfg.secrets_broken && <span className="mcp-scope-badge" style={{ color: 'var(--kr-warning)', borderColor: 'rgba(var(--kr-warning-rgb), 0.3)' }} title={t('mcp.secretsBroken')}>⚠ {t('mcp.secretsBrokenShort')}</span>}
                        <PluginKindBadge kind={cfgKind} />
                        {supportsHostSync && <HostSyncChip mode={cfg.host_sync} />}
                      </div>
                    </div>
                  </div>
                </div>
              );

              if (!isSelected) return [card];

              /* ── Inline detail: spans full grid width, right after this card ── */
              const def = mcpRegistry.find(m => m.id === cfg.server_id);
              const isEditingLabel = editingLabelId === cfg.id;
              const serverIncomp = mcpOverview.incompatibilities.filter(i => i.server_id === cfg.server_id);

              // 0.8.6 (#29) — open the edit form pre-filled with the
              // current Custom plugin's spec. Shared between the Edit
              // button (header) and the autodiscovery banner (body)
              // that surfaces on plugins with empty endpoints.
              const openEditCustomPlugin = async () => {
                if (!cfgServer?.api_spec) return;
                const spec = cfgServer.api_spec;
                setEditingCustomServerId(cfg.server_id);
                setEditingCustomConfigId(cfg.id);
                setCustomName(cfgServer.name);
                setCustomBaseUrl(spec.base_url);
                setCustomDescription(cfgServer.description);
                setCustomDocsUrl(spec.docs_url ?? '');
                const ck = spec.config_keys ?? [];
                let revealedEnv: Record<string, string> = {};
                try {
                  const entries = await mcpsApi.revealSecrets(cfg.id);
                  entries.forEach(e => { revealedEnv[e.key] = e.masked_value; });
                } catch (e) {
                  console.warn('Failed to reveal secrets for edit prefill:', e);
                }
                setCustomFields(
                  ck.length > 0
                    ? ck.map(k => ({
                        label: k.label,
                        value: revealedEnv[k.env_key] ?? '',
                      }))
                    : [{ label: '', value: '' }],
                );
                setCustomEndpoints(spec.endpoints ?? []);
                setCustomAuth(spec.auth ?? 'None');
                setShowAddMcp(true);
                setAddMcpSelected('api-custom');
                setSelectedConfigId(null);
                requestAnimationFrame(() => {
                  window.scrollTo({ top: 0, behavior: 'smooth' });
                });
              };

              // 0.8.6 (#29) — surface a banner on legacy Custom plugins
              // (created before 0.8.6, OR after but with no endpoints
              // declared yet) prompting the user to ask the AI helper
              // to fill them. Detection : the plugin's `server_id`
              // starts with `custom-` AND the api_spec has zero
              // declared endpoints. Banner CTA reuses
              // `openEditCustomPlugin` so the AI helper is one click
              // away (cf. [[project_endpoints_autodiscovery_0_8_6]]).
              const isLegacyCustomNoEndpoints =
                cfg.server_id.startsWith('custom-')
                && cfgServer?.api_spec
                && (cfgServer.api_spec.endpoints?.length ?? 0) === 0;
              const detail = (
                <div key={`detail-${cfg.id}`} ref={detailRef} className="mcp-detail-inline" onClick={e => e.stopPropagation()}>
                  <div className="mcp-detail-header">
                    <div className="mcp-registry-card-icon" style={{ width: 40, height: 40 }}><Puzzle size={20} /></div>
                    <div className="flex-1">
                      {isEditingLabel ? (
                        <input className="input mcp-detail-name-input" value={editingLabelText} onChange={e => setEditingLabelText(e.target.value)} onBlur={() => handleSaveLabel(cfg.id)} onKeyDown={e => { if (e.key === 'Enter') handleSaveLabel(cfg.id); if (e.key === 'Escape') setEditingLabelId(null); }} autoFocus />
                      ) : (
                        <h2 className="mcp-detail-name" onClick={() => { setEditingLabelId(cfg.id); setEditingLabelText(cfg.label); }}>{cfg.label} <Pencil size={11} className="text-ghost" /></h2>
                      )}
                      {def?.description && <p className="mcp-detail-desc">{def.description}</p>}
                      {def && <span className={`mcp-origin-badge ${def.official ? 'mcp-origin-official' : 'mcp-origin-community'}`}>
                        {def.official ? t('mcp.official') : t('mcp.community')} — {def.publisher}
                      </span>}
                      {serverIncomp.length > 0 && <span className="mcp-server-incompat">{serverIncomp.map(i => `⚠ ${i.agent}: ${i.reason}`).join(' · ')}</span>}
                    </div>
                    <div className="flex-row gap-3">
                      {/* 0.8.6 — Edit spec button. Only on Custom API plugins
                          (`custom-{slug}-{nano}` ids). Opens the same form as
                          create, pre-filled from the server's api_spec, then
                          PUTs `/api/mcps/custom/:server_id` on submit. The
                          encrypted env per-config is NOT edited here — the
                          user uses the existing "Edit env" drawer for that.
                          Closes the misleading "delete + recreate" gap surfaced
                          by the user 2026-05-19 on Didomi. */}
                      {cfg.server_id.startsWith('custom-') && cfgServer?.api_spec && (
                        <button
                          className="mcp-btn-action"
                          onClick={openEditCustomPlugin}
                          title={t('mcp.custom.editSpec')}
                        >
                          <Pencil size={12} /> {t('mcp.custom.editSpec')}
                        </button>
                      )}
                      {/* 0.8.6 (#33) — Export as JSON. Spec-only, no
                          credentials. Sharing the resulting payload is
                          safe. */}
                      {cfg.server_id.startsWith('custom-') && cfgServer?.api_spec && (
                        <button
                          className="mcp-btn-action"
                          onClick={() => handleExportCustomPlugin(cfgServer as McpServer)}
                          title={t('mcp.custom.copyAsJson')}
                          data-testid="mcp-custom-export-json"
                        >
                          <Upload size={12} /> {t('mcp.custom.copyAsJson')}
                        </button>
                      )}
                      <button className="mcp-btn-action" style={{ color: 'var(--kr-error)', borderColor: 'rgba(var(--kr-error-rgb), 0.3)' }} onClick={() => { handleDeleteMcpConfig(cfg.id); setSelectedConfigId(null); }}><Trash2 size={12} /> {t('mcp.deleteConfig')}</button>
                      <button className="mcp-icon-btn" onClick={() => setSelectedConfigId(null)} aria-label="Close"><X size={14} /></button>
                    </div>
                  </div>
                  <div className="mcp-detail-body">
                    {/* 0.8.6 (#29) — autodiscovery banner for legacy
                        Custom plugins with no endpoints declared. CTA
                        opens the same edit form as the header button,
                        where the CustomApiAiHelper (0.8.6 Part B) is
                        wired to fetch the docs_url + propose endpoints
                        via KRONN:APPLY. */}
                    {isLegacyCustomNoEndpoints && (
                      <div className="mcp-autodiscovery-banner" data-testid="mcp-autodiscovery-banner">
                        <Info size={14} className="mcp-autodiscovery-banner-icon" />
                        <div className="mcp-autodiscovery-banner-body">
                          <strong>{t('mcp.custom.autodiscoveryTitle')}</strong>
                          <p>{t('mcp.custom.autodiscoveryHint')}</p>
                        </div>
                        <button
                          type="button"
                          className="mcp-btn-action mcp-autodiscovery-banner-cta"
                          onClick={openEditCustomPlugin}
                        >
                          <Sparkles size={12} /> {t('mcp.custom.autodiscoveryCta')}
                        </button>
                      </div>
                    )}
                    {(() => {
                      // 0.8.6 — for Custom plugins, the SPEC's config_keys is
                      // the forward-looking source of truth (follows rename via
                      // Edit plugin). The stored `cfg.env_keys` may still
                      // carry orphan slugs from before a rename — surfacing
                      // them as the editable list would re-create them on
                      // save and confuse the user.  Registry plugins always
                      // agree (spec = env), so this is a no-op for them.
                      const isCustom = cfg.server_id.startsWith('custom-');
                      // 0.8.6 unified-edit (2026-05-20) — Custom plugins
                      // get a READ-ONLY env section (slugs + eye reveal,
                      // no edit button). The actual editing lives in
                      // "Modifier le plugin". User asked 2026-05-20 to
                      // be able to SEE stored values without entering
                      // edit mode ("Au pire sur la card de l'API on peut
                      // toujours afficher les variables, avec le petit
                      // oeil, SANS l'édition"). Registry plugins keep
                      // the editable section (their only env path).
                      if (isCustom) {
                        if (cfg.env_keys.length === 0) return null;
                        return (
                          <div className="mcp-detail-section">
                            <h3 className="mcp-detail-section-title">
                              <Key size={12} /> {t('mcp.envVars')}
                            </h3>
                            <p className="mcp-env-key-desc mb-3" style={{ fontSize: '0.85em' }}>
                              {t('mcp.custom.envViewOnlyHint')}
                            </p>
                            {cfg.env_keys.map(k => (
                              <div key={k} className="mcp-detail-field">
                                <label className="mcp-detail-field-label">{k}</label>
                                <div className="flex-row gap-3">
                                  <input
                                    className="input mcp-input-mono flex-1"
                                    value={editingEnvId === cfg.id ? (editingEnv[k] ?? '') : '••••••••'}
                                    type={editingEnvId === cfg.id && visibleFields.has(k) ? 'text' : 'password'}
                                    readOnly
                                    onChange={() => {}}
                                  />
                                  <button
                                    className="mcp-icon-btn"
                                    onClick={async () => {
                                      if (editingEnvId !== cfg.id) {
                                        const ok = await handleStartEditSecrets(cfg.id);
                                        if (!ok) return;
                                        setVisibleFields(prev => new Set(prev).add(k));
                                      } else {
                                        toggleFieldVisibility(k);
                                      }
                                    }}
                                    title={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}
                                  >
                                    <Eye size={12} style={{ color: visibleFields.has(k) ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} />
                                  </button>
                                </div>
                              </div>
                            ))}
                          </div>
                        );
                      }
                      const specKeys: string[] = cfgServer?.api_spec?.config_keys?.map(ck => ck.env_key) ?? [];
                      const displayEnvKeys = cfg.env_keys;
                      const orphanEnvKeys: string[] = [];
                      const hasAnything = displayEnvKeys.length > 0 || def?.token_help;
                      // Suppress unused-var warnings for the registry path.
                      void specKeys; void orphanEnvKeys;
                      return hasAnything ? (
                      <div className="mcp-detail-section">
                        <h3 className="mcp-detail-section-title"><Key size={12} /> {displayEnvKeys.length > 0 ? t('mcp.envVars') : t('mcp.setup')} {displayEnvKeys.length > 0 && editingEnvId !== cfg.id && <button className="mcp-icon-btn" style={{ marginLeft: 4 }} onClick={() => handleStartEditSecrets(cfg.id)} title={t('mcp.editKeys')} aria-label={t('mcp.editKeys')}><Pencil size={11} style={{ color: 'var(--kr-text-dim)' }} /></button>}</h3>
                        {def?.token_help && (() => {
                          const helpKey = `mcp.help.${def.id}`;
                          const translated = t(helpKey);
                          const helpText = translated !== helpKey ? translated : def.token_help;
                          return <p className="mcp-detail-field-label" style={{ whiteSpace: 'pre-wrap' }}>{linkify(helpText)}</p>;
                        })()}
                        {def?.token_url && <a href={def.token_url} target="_blank" rel="noopener noreferrer" className="mcp-secrets-token-link mb-4"><ExternalLink size={10} /> {t('mcp.getToken')}</a>}
                        {orphanEnvKeys.length > 0 && (
                          <p className="mcp-env-key-desc mb-3" style={{ color: 'var(--kr-warning)', borderLeft: '3px solid var(--kr-warning)', paddingLeft: '0.6rem' }}>
                            ⚠ {t('mcp.envOrphanHint', orphanEnvKeys.join(', '))}
                          </p>
                        )}
                        {displayEnvKeys.map(k => (
                          <div key={k} className="mcp-detail-field">
                            <label className="mcp-detail-field-label">{k}</label>
                            <div className="flex-row gap-3">
                              <input className="input mcp-input-mono flex-1" value={editingEnvId === cfg.id ? (editingEnv[k] ?? '') : '••••••••'} onChange={e => setEditingEnv(prev => ({ ...prev, [k]: e.target.value }))} type={editingEnvId === cfg.id && visibleFields.has(k) ? 'text' : 'password'} placeholder={t('mcp.value')} readOnly={editingEnvId !== cfg.id} onClick={() => { if (editingEnvId !== cfg.id) handleStartEditSecrets(cfg.id); }} />
                              <button className="mcp-icon-btn" onClick={async () => { if (editingEnvId !== cfg.id) { const ok = await handleStartEditSecrets(cfg.id); if (!ok) return; setVisibleFields(prev => new Set(prev).add(k)); } else { toggleFieldVisibility(k); } }} title={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}><Eye size={12} style={{ color: visibleFields.has(k) ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} /></button>
                            </div>
                          </div>
                        ))}
                        {editingEnvError && editingEnvId === cfg.id && (
                          <div className="mcp-env-warning" style={{ color: 'var(--kr-warning)', fontSize: '0.8rem', marginTop: 6 }}>{editingEnvError}</div>
                        )}
                        {editingEnvId === cfg.id && (
                          <div className="flex-row gap-3 mt-4">
                            <button className="mcp-btn-action mcp-btn-action-primary" onClick={handleSaveSecrets} disabled={editingEnvLoading}><Save size={12} /> {editingEnvLoading ? t('mcp.saving') : t('mcp.save')}</button>
                            <button className="mcp-btn-action" onClick={() => setEditingEnvId(null)}>{t('mcp.cancel')}</button>
                          </div>
                        )}
                      </div>
                      ) : null;
                    })()}
                    <div className="mcp-detail-section">
                      <h3 className="mcp-detail-section-title">{t('mcp.scope')}</h3>
                      <div className="mcp-toggle-row">
                        <span className={`mcp-toggle-label mcp-toggle-global${cfg.is_global ? ' mcp-toggle-global-active' : ''}`} onClick={() => handleToggleConfigGlobal(cfg)} title={cfg.is_global ? t('mcp.disableGlobal') : t('mcp.enableGlobal')}>Global</span>
                        <span className={`mcp-toggle-label mcp-toggle-general${cfg.include_general ? ' mcp-toggle-general-active' : ''}`} onClick={async () => { try { await mcpsApi.updateConfig(cfg.id, { include_general: !cfg.include_general }); refetchMcps(); } catch (e) { console.warn(e); } }} title={cfg.include_general ? t('mcp.disableGeneral') : t('mcp.enableGeneral')}>{t('mcp.general')}</span>
                      </div>
                      <div className="mcp-toggle-row">
                        {(() => {
                          const sorted = projects.filter(p => !isHiddenPath(p.path)).sort((a, b) => {
                            const aL = (cfg.is_global || cfg.project_ids.includes(a.id)) ? 0 : 1;
                            const bL = (cfg.is_global || cfg.project_ids.includes(b.id)) ? 0 : 1;
                            return aL - bL || a.name.localeCompare(b.name);
                          });
                          const showAll = expandedProjectLists.has(cfg.id);
                          const visible = showAll ? sorted : sorted.slice(0, PROJECT_TOGGLE_LIMIT);
                          const hiddenCount = sorted.length - visible.length;
                          return (<>
                            {visible.map(proj => {
                              const isLinked = cfg.is_global || cfg.project_ids.includes(proj.id);
                              const projMcpCount = mcpOverview.configs.filter(c => c.is_global || c.project_ids.includes(proj.id)).length;
                              const loadClass = projMcpCount <= 5 ? 'mcp-load-ok' : projMcpCount <= 10 ? 'mcp-load-warn' : 'mcp-load-danger';
                              const loadTitle = projMcpCount <= 5 ? t('mcp.mcpLoadOk') : projMcpCount <= 10 ? t('mcp.mcpLoadWarn') : t('mcp.mcpLoadDanger');
                              return (
                                <span key={proj.id} className="flex-row">
                                  <button className={`mcp-project-toggle ${isLinked ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`} onClick={() => handleToggleConfigProject(cfg.id, proj.id, isLinked)}>
                                    {isLinked ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                                    {proj.name}
                                    <span className={`mcp-load-badge ${loadClass}`} title={loadTitle}>{projMcpCount}</span>
                                  </button>
                                  {isLinked && (() => {
                                    const slug = slugify(cfg.label);
                                    const isCustom = mcpOverview.customized_contexts.includes(`${slug}:${proj.id}`);
                                    return <button className="mcp-icon-btn mcp-context-btn" onClick={() => handleOpenContext(proj.id, proj.name, cfg.label)} title={`${t('mcp.editContext', cfg.label, proj.name)}${isCustom ? ' ' + t('mcp.customized') : ' ' + t('mcp.default')}`}><FileText size={10} style={{ color: isCustom ? 'var(--kr-accent)' : 'var(--kr-text-ghost)' }} /></button>;
                                  })()}
                                </span>
                              );
                            })}
                            {hiddenCount > 0 && <button className="mcp-more-projects-btn" onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.add(cfg.id); return n; })}>{t('mcp.moreProjects', hiddenCount)}</button>}
                            {showAll && sorted.length > PROJECT_TOGGLE_LIMIT && <button className="mcp-less-projects-btn" onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.delete(cfg.id); return n; })}>{t('mcp.lessProjects')}</button>}
                          </>);
                        })()}
                      </div>
                    </div>
                    {/* ── Sync CLIs locaux (Phase-3 refactor — checkbox dans Scope) ──
                        Hidden entirely for API-only plugins: those don't have
                        an MCP transport to write to `.mcp.json` / Codex / Gemini
                        / Copilot, they only exist as a `## REST APIs available`
                        block in the agent's system prompt. Showing a "Sync CLI"
                        toggle on them was misleading — the user reported the
                        confusion. Hybrid plugins keep the toggle but get a
                        note that it only affects the MCP side. */}
                    {supportsHostSync && (
                    <div
                      className="mcp-host-sync-block"
                      style={{ marginTop: 12, paddingTop: 12, borderTop: '1px dashed var(--kr-border, #e5e7eb)', position: 'relative' }}
                    >
                      <label
                        style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', fontSize: '0.95em', fontWeight: 500 }}
                      >
                        <input
                          type="checkbox"
                          checked={cfg.host_sync !== 'None'}
                          onChange={(e) => handleSetHostSync(cfg.id, e.target.checked ? 'GlobalOnly' : 'None')}
                        />
                        <Globe size={13} />
                        Aussi disponible dans mes CLIs locaux
                      </label>
                      {cfgKind === 'hybrid' && (
                        <p className="text-muted" style={{ fontSize: '0.8em', margin: '4px 0 0 22px', fontStyle: 'italic' }}>
                          Plugin hybride : la sync CLI s'applique uniquement à la partie MCP. La partie API (endpoints REST) est toujours injectée dans le prompt agent — elle n'est jamais écrite dans tes fichiers home.
                        </p>
                      )}
                      {cfg.host_sync !== 'None' && (
                        <HostSyncPreview
                          isGlobal={cfg.is_global}
                          projectIds={cfg.project_ids}
                          projects={projects}
                        />
                      )}
                      <PorteeCliCoachMark />
                    </div>
                    )}
                    {/* For API-only plugins: tell the user explicitly that
                        the toggle they would expect here doesn't apply. */}
                    {!supportsHostSync && (
                      <div
                        style={{ marginTop: 12, paddingTop: 12, borderTop: '1px dashed var(--kr-border, #e5e7eb)' }}
                      >
                        <p className="text-muted" style={{ fontSize: '0.85em', margin: 0 }}>
                          <Globe size={11} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
                          Plugin API : pas de sync CLI locale. Les endpoints REST de ce plugin sont injectés directement dans le prompt système de l'agent (avec exemples curl + auth). Il n'y a aucun fichier <code>.mcp.json</code> / <code>~/.codex/config.toml</code> / etc. à mettre à jour.
                        </p>
                      </div>
                    )}
                  </div>
                </div>
              );
              return [card, detail];
            })}
        </div>
      ) : !showAddMcp ? (
        <div className="mcp-card mcp-empty">
          <Puzzle size={32} className="text-ghost mb-6" />
          <p className="mcp-empty-text">
            {t('mcp.empty')}
          </p>
        </div>
      ) : null}

      {/* ── MCP Context Editor Modal ── */}
      {contextEditor && (
        <div className="mcp-modal-overlay" onClick={() => setContextEditor(null)}>
          <div
            className="mcp-modal"
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="context-editor-title"
            onKeyDown={e => { if (e.key === 'Escape') setContextEditor(null); }}
          >
            <div className="flex-between">
              <div>
                <h3 id="context-editor-title" className="mcp-modal-title">
                  <FileText size={14} className="text-accent" style={{ marginRight: 6 }} />
                  {t('mcp.contextTitle', contextEditor.slug.replace(/-/g, ' '))}
                </h3>
                <p className="mcp-modal-subtitle">
                  {t('mcp.contextInfo', contextEditor.projectName, contextEditor.slug)}
                </p>
              </div>
              <button className="mcp-icon-btn" onClick={() => setContextEditor(null)} aria-label="Close"><X size={14} /></button>
            </div>

            <textarea
              className="input mcp-modal-textarea"
              value={contextEditor.content}
              onChange={e => setContextEditor(prev => prev ? { ...prev, content: e.target.value } : null)}
              placeholder={t('mcp.contextPlaceholder')}
            />

            <div className="flex-row gap-4" style={{ justifyContent: 'flex-end' }}>
              <button className="mcp-btn-action" onClick={() => setContextEditor(null)}>{t('mcp.cancel')}</button>
              <button
                className="mcp-btn-action mcp-btn-action-primary"
                onClick={handleSaveContext}
                disabled={contextSaving}
              >
                {contextSaving ? t('mcp.saving') : t('mcp.save')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Coach mark for the first-ever open of a Plugin drawer after the host
 * sync feature was added. Appears once, dismissible, persisted in
 * localStorage. Sits above the Portée CLI radio group with a subtle
 * arrow / badge so existing users discover the feature without it
 * being intrusive on subsequent edits.
 */
const PORTEE_CLI_COACH_KEY = 'kronn:portee-cli-coach-seen';

function PorteeCliCoachMark() {
  const [dismissed, setDismissed] = useState(() => {
    try { return localStorage.getItem(PORTEE_CLI_COACH_KEY) === '1'; } catch { return true; }
  });

  if (dismissed) return null;

  const dismiss = () => {
    try { localStorage.setItem(PORTEE_CLI_COACH_KEY, '1'); } catch { /* incognito / quota — coach reappears on next session */ }
    setDismissed(true);
  };

  return (
    <div
      style={{
        position: 'absolute',
        top: -2,
        right: 0,
        background: 'var(--kr-accent, #3b82f6)',
        color: '#fff',
        padding: '6px 10px',
        borderRadius: 4,
        fontSize: '0.78em',
        fontWeight: 500,
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        boxShadow: '0 2px 8px rgba(59, 130, 246, 0.3)',
        maxWidth: 280,
        zIndex: 5,
      }}
    >
      <span>✨ Nouveau : expose ce MCP à tes CLIs locaux ici</span>
      <button
        onClick={dismiss}
        style={{
          all: 'unset',
          cursor: 'pointer',
          padding: 2,
          opacity: 0.85,
          flexShrink: 0,
        }}
        title="OK, j'ai compris"
      >
        <X size={11} />
      </button>
    </div>
  );
}

/**
 * Empty-state hint shown when the user has MCPs configured but none of
 * them is exposed to local CLIs (host_sync === 'None'). Surfaces the
 * "Portée CLI locale" feature without nagging users who deliberately
 * keep everything Kronn-only — dismissible via localStorage.
 */
const CLI_EXPOSURE_HINT_DISMISS_KEY = 'kronn:cli-exposure-hint-dismissed';

function CliExposureHint({ configs, onJumpToConfig }: { configs: McpConfigDisplay[]; onJumpToConfig: (id: string) => void }) {
  const [dismissed, setDismissed] = useState(() => {
    try { return localStorage.getItem(CLI_EXPOSURE_HINT_DISMISS_KEY) === '1'; } catch { return false; }
  });

  // Show only when : at least 1 MCP exists AND none is exposed in CLI
  const noneExposed = configs.length > 0 && configs.every(c => c.host_sync === 'None');
  if (dismissed || !noneExposed) return null;

  const dismiss = () => {
    try { localStorage.setItem(CLI_EXPOSURE_HINT_DISMISS_KEY, '1'); } catch { /* incognito / quota — hint reappears on next session */ }
    setDismissed(true);
  };

  // Pick the first config alphabetically as the "jump-to" target
  const firstConfig = [...configs].sort((a, b) => a.label.localeCompare(b.label))[0];

  return (
    <div
      style={{
        margin: '12px 0',
        padding: '12px 16px',
        border: '1px solid var(--kr-border, #e5e7eb)',
        borderLeft: '3px solid var(--kr-accent, #3b82f6)',
        borderRadius: 6,
        background: 'var(--kr-info-bg, rgba(96, 165, 250, 0.04))',
        fontSize: '0.9em',
        display: 'flex',
        alignItems: 'flex-start',
        gap: 10,
      }}
    >
      <Globe size={16} style={{ color: 'var(--kr-accent, #3b82f6)', flexShrink: 0, marginTop: 2 }} />
      <div style={{ flex: 1 }}>
        <strong>Aucun MCP n'est exposé en CLI hors Kronn.</strong>
        <div className="text-muted" style={{ marginTop: 4, fontSize: '0.9em' }}>
          Si tu utilises Claude Code / Gemini / Codex en dehors de Kronn et veux y avoir accès aux mêmes MCPs,
          ouvre une config et choisis "Dans Kronn + CLIs locaux" dans la section Portée CLI.
        </div>
        {firstConfig && (
          <button
            className="btn btn-xs"
            onClick={() => onJumpToConfig(firstConfig.id)}
            style={{ marginTop: 8 }}
          >
            Configurer "{firstConfig.label}" →
          </button>
        )}
      </div>
      <button
        className="btn btn-xs btn-ghost"
        onClick={dismiss}
        title="Ne plus afficher"
        style={{ flexShrink: 0 }}
      >
        <X size={12} />
      </button>
    </div>
  );
}
