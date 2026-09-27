import type { RedactedField } from '../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** The fields an export masked, read defensively from any parsed bundle. */
export function redactedFields(parsed: unknown): RedactedField[] {
  const list = (parsed as { redacted_fields?: unknown } | null)?.redacted_fields;
  if (!Array.isArray(list)) return [];
  return list.filter((item): item is RedactedField =>
    typeof item === 'object' && item !== null
    && typeof (item as RedactedField).name === 'string'
    && typeof (item as RedactedField).field === 'string');
}

/** Same as `redactedFields`, from the raw file text; empty when it is not JSON. */
export function redactedFieldsIn(content: string): RedactedField[] {
  try {
    return redactedFields(JSON.parse(content));
  } catch {
    return [];
  }
}

export function describeRedacted(field: RedactedField, t: Translate): string {
  const kind = t(`imp.redactedKind.${field.kind}`);
  return `${kind} « ${field.name} » : ${field.field}`;
}

/** Notice to show after an export download, or null when nothing was masked. */
export async function exportRedactionNotice(blob: Blob, t: Translate): Promise<string | null> {
  const fields = redactedFieldsIn(await blob.text());
  if (!fields.length) return null;
  const listed = fields.slice(0, 3).map(field => describeRedacted(field, t)).join(' ; ');
  const more = fields.length > 3 ? ` (+${fields.length - 3})` : '';
  return t('imp.exportRedacted', fields.length, `${listed}${more}`);
}
