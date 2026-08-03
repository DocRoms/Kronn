import type { McpServer, WorkflowStep } from '../../types/generated';
import {
  managedHeaderNames,
  managedQueryNames,
  stripManagedHeaders,
  stripManagedQuery,
} from './apiCallAuth';

type Translator = (key: string, ...args: (string | number)[]) => string;

export interface ApplySuggestion {
  signature: string;
  parsed: Record<string, unknown>;
  applied: boolean;
}

export const KRONN_APPLY_RX = /KRONN:APPLY\s*```json\s*([\s\S]*?)```/g;

export function parseApplyBlocks(text: string): ApplySuggestion[] {
  const suggestions: ApplySuggestion[] = [];
  for (const match of text.matchAll(KRONN_APPLY_RX)) {
    try {
      const parsed = JSON.parse(match[1]) as Record<string, unknown>;
      suggestions.push({ signature: match[1].trim(), parsed, applied: false });
    } catch {
      // Streaming may expose an incomplete block; wait for the next chunk.
    }
  }
  return suggestions;
}

export function applyToStep(
  parsed: Record<string, unknown>,
  step: WorkflowStep,
  server: McpServer | null = null,
): Partial<WorkflowStep> {
  const updates: Partial<WorkflowStep> = {};
  const managedQuery = managedQueryNames(server);
  const managedHeaders = managedHeaderNames(server);

  if (typeof parsed.endpoint === 'string') updates.api_endpoint_path = parsed.endpoint;
  if (typeof parsed.method === 'string') updates.api_method = parsed.method.toUpperCase();
  if (parsed.query && typeof parsed.query === 'object' && !Array.isArray(parsed.query)) {
    const raw: Record<string, string> = {};
    for (const [key, value] of Object.entries(parsed.query as Record<string, unknown>)) {
      raw[key] = typeof value === 'string' ? value : JSON.stringify(value);
    }
    updates.api_query = stripManagedQuery(raw, managedQuery);
  }
  if (parsed.headers && typeof parsed.headers === 'object' && !Array.isArray(parsed.headers)) {
    const raw: Record<string, string> = {};
    for (const [key, value] of Object.entries(parsed.headers as Record<string, unknown>)) {
      raw[key] = typeof value === 'string' ? value : JSON.stringify(value);
    }
    updates.api_headers = stripManagedHeaders(raw, managedHeaders);
  }
  if (parsed.body !== undefined && parsed.body !== null) {
    updates.api_body = typeof parsed.body === 'string' ? parsed.body : JSON.stringify(parsed.body);
  }
  if (typeof parsed.extract === 'string') {
    updates.api_extract = {
      path: parsed.extract,
      fallback: step.api_extract?.fallback ?? null,
      fail_on_empty: step.api_extract?.fail_on_empty ?? false,
    };
  }
  return updates;
}

function truncateJson(value: unknown, max = 1500): string {
  const serialized = JSON.stringify(value, null, 2);
  if (serialized == null) return '(null)';
  return serialized.length > max
    ? `${serialized.slice(0, max)}\n… [truncated, ${serialized.length - max} chars omitted]`
    : serialized;
}

export function buildContextBlock(
  server: McpServer | null,
  step: WorkflowStep,
  lastTestResponse?: unknown,
  lastTestError?: string | null,
  t?: Translator,
): string {
  const translate: Translator = t ?? ((key: string) => key);
  const lines = [
    translate('wf.apicall.helper.sys.ctxHeader'),
    `- API : ${server?.name ?? translate('wf.apicall.helper.sys.noPlugin')} (${server?.api_spec?.base_url ?? translate('wf.apicall.helper.sys.unknown')})`,
    `- endpoint : ${step.api_endpoint_path ?? translate('wf.apicall.helper.sys.none')}`,
    `- method   : ${step.api_method ?? translate('wf.apicall.helper.sys.default')}`,
    `- query    : ${step.api_query ? JSON.stringify(step.api_query) : translate('wf.apicall.helper.sys.empty')}`,
    `- headers  : ${step.api_headers ? JSON.stringify(step.api_headers) : translate('wf.apicall.helper.sys.none')}`,
    `- body     : ${step.api_body || translate('wf.apicall.helper.sys.none')}`,
    `- extract  : ${step.api_extract?.path ?? translate('wf.apicall.helper.sys.none')}`,
  ];

  if (lastTestError) {
    lines.push('', translate('wf.apicall.helper.sys.ctxLastFail'), lastTestError);
  } else if (lastTestResponse !== undefined && lastTestResponse !== null) {
    lines.push(
      '',
      translate('wf.apicall.helper.sys.ctxLastOk'),
      '```json',
      truncateJson(lastTestResponse),
      '```',
    );
  }
  return lines.join('\n');
}
