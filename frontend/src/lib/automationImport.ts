export type AutomationImportKind = 'workflow' | 'qp' | 'qa' | 'qe';

export type AutomationImportDetection =
  | { ok: true; kind: AutomationImportKind }
  | { ok: false; reason: 'ambiguous' | 'invalid' };

const EXPORT_SHAPES = [
  { kind: 'workflow', discriminator: 'kronn.workflow', payloadKey: 'workflow' },
  { kind: 'qp', discriminator: 'kronn.quick_prompt', payloadKey: 'quick_prompt' },
  { kind: 'qa', discriminator: 'kronn.quick_api', payloadKey: 'quick_api' },
  { kind: 'qe', discriminator: 'kronn.quick_exec', payloadKey: 'quick_exec' },
] as const;

/**
 * Detect a portable Automation export without guessing from filenames or
 * whichever sidebar section happens to be open. The discriminator remains
 * authoritative, while the unique payload key protects the global importer
 * from accepting a mixed/ambiguous envelope that the backend would reject.
 */
export function detectAutomationImport(
  parsed: Record<string, unknown>,
): AutomationImportDetection {
  const discriminatorShape = EXPORT_SHAPES.find(
    shape => shape.discriminator === parsed.kind,
  );
  const payloadShapes = EXPORT_SHAPES.filter(
    shape => Object.prototype.hasOwnProperty.call(parsed, shape.payloadKey),
  );

  const signalledKinds = new Set<AutomationImportKind>(payloadShapes.map(shape => shape.kind));
  if (discriminatorShape) signalledKinds.add(discriminatorShape.kind);

  if (signalledKinds.size > 1) return { ok: false, reason: 'ambiguous' };
  if (!discriminatorShape) return { ok: false, reason: 'invalid' };

  const payload = parsed[discriminatorShape.payloadKey];
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload)) {
    return { ok: false, reason: 'invalid' };
  }

  return { ok: true, kind: discriminatorShape.kind };
}
