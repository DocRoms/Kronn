import type { ApiEndpoint, CustomApiField, CustomApiPayload } from '../types/generated';

export type Translator = (key: string, ...args: (string | number)[]) => string;

export interface CustomApiFormSnapshot {
  name: string;
  base_url: string;
  description: string;
  docs_url: string;
  fields: CustomApiField[];
  endpoints: ApiEndpoint[];
}

export function buildSystemPrompt(t: Translator): string {
  return `${t('mcp.custom.helper.sys.role')}

${t('mcp.custom.helper.sys.boundaries')}

${t('mcp.custom.helper.sys.action')}

${t('mcp.custom.helper.sys.format')}

KRONN:APPLY
\`\`\`json
{
  "name": "Salesforce Sales API",
  "base_url": "https://my-org.salesforce.com/services/data/v59.0",
  "description": "REST API for Salesforce Sales objects (Account, Contact, Opportunity)",
  "docs_url": "https://developer.salesforce.com/docs/atlas.en-us.api_rest.meta/api_rest/",
  "fields": [
    {"label": "Bearer Token", "value": ""},
    {"label": "Org ID", "value": ""}
  ],
  "endpoints": [
    {"path": "/sobjects/Account", "method": "GET", "description": "List accounts"},
    {"path": "/sobjects/Contact", "method": "POST", "description": "Create contact"},
    {"path": "/query", "method": "GET", "description": "SOQL query"}
  ]
}
\`\`\`

${t('mcp.custom.helper.sys.endpoints')}

${t('mcp.custom.helper.sys.verify')}

${t('mcp.custom.helper.sys.partial')}

${t('mcp.custom.helper.sys.style')}

${t('mcp.custom.helper.sys.starter')}`;
}

export function buildContextBlock(snapshot: CustomApiFormSnapshot, t: Translator): string {
  const fieldsLine = snapshot.fields.length === 0
    ? t('mcp.custom.helper.ctx.noFields')
    : snapshot.fields.map(f => `  - ${f.label || '(blank)'}${f.value ? ' ✓' : ' (empty)'}`).join('\n');
  const endpointsLine = snapshot.endpoints.length === 0
    ? t('mcp.custom.helper.ctx.noEndpoints')
    : snapshot.endpoints.slice(0, 5).map(e => `  - ${e.method} ${e.path}`).join('\n')
      + (snapshot.endpoints.length > 5 ? `\n  - … (+${snapshot.endpoints.length - 5})` : '');
  return `${t('mcp.custom.helper.ctx.header')}
- name        : ${snapshot.name || t('mcp.custom.helper.ctx.empty')}
- base_url    : ${snapshot.base_url || t('mcp.custom.helper.ctx.empty')}
- description : ${snapshot.description || t('mcp.custom.helper.ctx.empty')}
- docs_url    : ${snapshot.docs_url || t('mcp.custom.helper.ctx.empty')}
- fields      :
${fieldsLine}
- endpoints   :
${endpointsLine}`;
}

export function applyToCustomForm(parsed: Record<string, unknown>): Partial<CustomApiPayload> {
  const updates: Partial<CustomApiPayload> = {};
  if (typeof parsed.name === 'string') updates.name = parsed.name;
  if (typeof parsed.base_url === 'string') updates.base_url = parsed.base_url;
  if (typeof parsed.description === 'string') updates.description = parsed.description;
  if (typeof parsed.docs_url === 'string') updates.docs_url = parsed.docs_url;
  if (Array.isArray(parsed.fields)) {
    const fields: CustomApiField[] = [];
    for (const raw of parsed.fields) {
      if (raw && typeof raw === 'object' && 'label' in raw) {
        const field = raw as Record<string, unknown>;
        if (typeof field.label === 'string' && field.label.trim()) {
          fields.push({
            label: field.label,
            value: typeof field.value === 'string' ? field.value : '',
          });
        }
      }
    }
    if (fields.length > 0) updates.fields = fields;
  }
  if (Array.isArray(parsed.endpoints)) {
    const endpoints: ApiEndpoint[] = [];
    for (const raw of parsed.endpoints) {
      if (raw && typeof raw === 'object' && 'path' in raw) {
        const endpoint = raw as Record<string, unknown>;
        if (typeof endpoint.path === 'string' && endpoint.path.trim()) {
          endpoints.push({
            path: endpoint.path.trim(),
            method: typeof endpoint.method === 'string' && endpoint.method.trim()
              ? endpoint.method.trim().toUpperCase()
              : 'GET',
            description: typeof endpoint.description === 'string' ? endpoint.description : '',
          });
        }
      }
    }
    if (endpoints.length > 0) updates.endpoints = endpoints;
  }
  return updates;
}
