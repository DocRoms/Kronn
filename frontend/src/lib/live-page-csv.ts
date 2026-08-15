import type { LivePageDatasetView } from '../types/generated';

export type ExportCell = string | number | boolean | null;
export type ExportRecord = Record<string, ExportCell>;

function flattenValue(value: unknown, prefix = '', target: ExportRecord = {}): ExportRecord {
  if (value === null || typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
    target[prefix || 'value'] = value as ExportCell;
  } else if (Array.isArray(value)) {
    target[prefix || 'value'] = JSON.stringify(value);
  } else if (typeof value === 'object') {
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      flattenValue(child, prefix ? `${prefix}.${key}` : key, target);
    }
  } else {
    target[prefix || 'value'] = String(value ?? '');
  }
  return target;
}

function rowValue(value: unknown, prefix = ''): ExportRecord {
  if (!Array.isArray(value)) return flattenValue(value, prefix);
  const record: ExportRecord = {};
  value.forEach((cell, index) => {
    flattenValue(cell, prefix ? `${prefix}.${index + 1}` : `column_${index + 1}`, record);
  });
  return record;
}

interface ArrayColumn {
  path: string;
  values: unknown[];
}

function collectColumns(
  value: unknown,
  prefix: string,
  scalars: ExportRecord,
  arrays: ArrayColumn[],
): void {
  if (Array.isArray(value)) {
    arrays.push({ path: prefix || 'value', values: value });
    return;
  }
  if (value !== null && typeof value === 'object') {
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      collectColumns(child, prefix ? `${prefix}.${key}` : key, scalars, arrays);
    }
    return;
  }
  flattenValue(value, prefix, scalars);
}

/**
 * Turns arbitrary retained JSON into a stable row set:
 * - a top-level array becomes one row per item;
 * - object envelopes expand their single array and repeat scalar metadata;
 * - parallel (including nested) arrays are zipped by index;
 * - plain objects and scalar values remain one row.
 */
export function valueRecords(value: unknown): ExportRecord[] {
  if (value === null || value === undefined) return [];
  if (Array.isArray(value)) return value.map(item => rowValue(item));
  if (typeof value !== 'object') return [rowValue(value)];

  const scalars: ExportRecord = {};
  const arrays: ArrayColumn[] = [];
  collectColumns(value, '', scalars, arrays);
  if (arrays.length === 0) return [scalars];

  const rowCount = Math.max(...arrays.map(column => column.values.length));
  if (rowCount === 0) return Object.keys(scalars).length > 0 ? [scalars] : [];
  const omitSingleArrayPrefix = arrays.length === 1;
  return Array.from({ length: rowCount }, (_, index) => {
    const record = { ...scalars };
    for (const column of arrays) {
      if (index >= column.values.length) continue;
      Object.assign(record, rowValue(
        column.values[index],
        omitSingleArrayPrefix ? '' : column.path,
      ));
    }
    return record;
  });
}

export function datasetRecords(dataset: LivePageDatasetView): ExportRecord[] {
  if (dataset.kind === 'time_series') {
    return dataset.points.flatMap(point => {
      const payloadRecords = valueRecords(point.payload);
      return (payloadRecords.length > 0 ? payloadRecords : [{}]).map(record => ({
        ...record,
        observed_at: point.observed_at,
        workflow_run_id: point.workflow_run_id,
      }));
    });
  }
  return valueRecords(dataset.current);
}

export function recordsToRows(records: ExportRecord[]): ExportCell[][] {
  const headers = [...new Set(records.flatMap(record => Object.keys(record)))];
  return headers.length === 0
    ? []
    : [headers, ...records.map(record => headers.map(header => record[header] ?? null))];
}
