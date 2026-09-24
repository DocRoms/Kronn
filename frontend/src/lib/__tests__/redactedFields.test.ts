import { describe, expect, it } from 'vitest';
import { describeRedacted, exportRedactionNotice, redactedFields } from '../redactedFields';

const t = (key: string, ...args: (string | number)[]) =>
  args.reduce<string>((text, arg, i) => text.replace(`{${i}}`, String(arg)), key === 'imp.exportRedacted' ? '{0} masked: {1}' : key);

const field = (name: string, path: string) => ({ kind: 'quick_api', resource_id: 'qa-1', name, field: path });

describe('redactedFields', () => {
  it('reads the list and drops malformed entries', () => {
    expect(redactedFields({ redacted_fields: [field('A', 'api_headers.Authorization'), { kind: 'x' }, null] }))
      .toEqual([field('A', 'api_headers.Authorization')]);
    expect(redactedFields({})).toEqual([]);
    expect(redactedFields(null)).toEqual([]);
  });

  it('describes a field by kind, resource and location', () => {
    expect(describeRedacted(field('Chartbeat', 'api_query.api_key'), t))
      .toBe('imp.redactedKind.quick_api « Chartbeat » : api_query.api_key');
  });
});

describe('exportRedactionNotice', () => {
  it('stays silent when nothing was masked or the file is not JSON', async () => {
    expect(await exportRedactionNotice(new Blob(['{"version":1}']), t)).toBeNull();
    expect(await exportRedactionNotice(new Blob(['not json']), t)).toBeNull();
  });

  it('counts every field and lists at most three', async () => {
    const fields = ['a', 'b', 'c', 'd'].map(n => field(n, `args.${n}`));
    const notice = await exportRedactionNotice(new Blob([JSON.stringify({ redacted_fields: fields })]), t);
    expect(notice).toMatch(/^4 masked: /);
    expect(notice).toContain('« c »');
    expect(notice).not.toContain('« d »');
    expect(notice).toMatch(/\(\+1\)$/);
  });
});
