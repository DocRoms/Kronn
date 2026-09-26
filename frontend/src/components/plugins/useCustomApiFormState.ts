import { useState } from 'react';
import type { ApiAuthKind, ApiEndpoint } from '../../types/generated';

/** Custom API / JSON plugin form fields: name/base URL/description/docs,
 *  key-value fields, endpoints and auth. Shared by the create form (Add
 *  MCP modal) and the edit form (plugin detail panel), both driven by
 *  the same `McpPageState`. Split out of the monolithic
 *  `useMcpPageState` (KT-830) to stay under the page's per-file line
 *  budget. */
export function useCustomApiFormState() {
  // Custom API form state. Only meaningful when addMcpSelected === 'api-custom'.
  // The shape mirrors `CustomApiPayload` in generated.ts so submit can
  // forward it as-is via `custom_spec`.
  const [customName, setCustomName] = useState('');
  const [customBaseUrl, setCustomBaseUrl] = useState('');
  const [customDescription, setCustomDescription] = useState('');
  const [customDocsUrl, setCustomDocsUrl] = useState('');
  // `stored` (edit mode only) = this field's env_key already has a value
  // saved in the config. Such fields render as a read-only masked "🔒 stored
  // + Remplacer" affordance (never a pre-filled or empty input), so the
  // user always sees that a key exists and a blank field never looks like
  // a wipe. See `replacingFields` for the "Remplacer" toggle.
  const [customFields, setCustomFields] = useState<Array<{ label: string; value: string; stored?: boolean }>>([
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
  // The id of the config whose encrypted env the "Modifier le plugin"
  // (single edit surface) save patches. Read in the submit handler.
  const [editingCustomConfigId, setEditingCustomConfigId] = useState<string | null>(null);
  // 0.8.6 — `editingStoredEnvKeys` removed 2026-05-20 when the edit
  // form's value column was replaced by a static "→ Édite via Éditer
  // les secrets" passive text. The per-field "•••• stocké" placeholder
  // is no longer needed (no input, no need to hint "still stored").
  // Orphan-env warning now lives on the env-edit drawer side (where it
  // belongs architecturally) rather than this spec-edit form.
  //
  // Show/hide of secret values + read-only peek are now owned by the shared
  // <SecretField> component (refonte 2026-06-09), so the page no longer holds
  // per-row visibility / revealed-value state. It only tracks which stored
  // fields the user chose to REPLACE (parent owns the replace decision):
  const [replacingFields, setReplacingFields] = useState<Set<number>>(new Set());
  // 0.8.6 — Custom plugin auth state. MVP exposes 3 variants out of
  // the 7 supported by the runtime: None (default), Bearer (simple
  // static token), TokenExchange (Didomi-shape: POST creds → access
  // token → Bearer). The other 4 (ApiKeyQuery / ApiKeyHeader / Basic /
  // BasicApiKey / OAuth2) come in 0.8.6 Layer A — they all work in
  // the runtime, just no UI yet. Cf. [[project_custom_plugin_auth_0_8_7]].
  const [customAuth, setCustomAuth] = useState<ApiAuthKind>('None');

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

  return {
    customName, setCustomName, customBaseUrl, setCustomBaseUrl,
    customDescription, setCustomDescription, customDocsUrl, setCustomDocsUrl,
    customFields, setCustomFields, customEndpoints, setCustomEndpoints,
    editingCustomServerId, setEditingCustomServerId, editingCustomConfigId, setEditingCustomConfigId,
    replacingFields, setReplacingFields, customAuth, setCustomAuth,
    authKindOf, setAuthKindBy, slugEnvKey,
  };
}
