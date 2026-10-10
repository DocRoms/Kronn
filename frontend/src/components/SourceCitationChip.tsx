// KT-609 — a citation you can read inside a sentence, and still verify.
//
// The exact reference stays on the chip's title, because a citation is
// evidence: if making it pretty makes it less consultable, this failed.
import { useT } from '../lib/I18nContext';
import type { SourceCheck } from '../types/generated';
import { chipStateFor, laneOf, type SourceCitation } from '../lib/sourceCitations';
import './SourceCitationChip.css';

/** What the backend already decided about this marker, when it decided. */
function verdictFor(
  citation: SourceCitation,
  sources: SourceCheck[] | undefined,
): SourceCheck | undefined {
  // `raw` is exactly what `anti_halluc.rs` stored, so this is an equality and
  // not a guess. A message from before the check existed simply has none.
  return sources?.find(source => source.raw === citation.raw);
}

export function SourceCitationChip({
  citation,
  sources,
}: {
  citation: SourceCitation;
  sources?: SourceCheck[];
}) {
  const { t } = useT();
  const verdict = verdictFor(citation, sources);
  // The kind is the one word a reader has to understand, so it is translated —
  // `code-comment` and `inferred` say nothing to someone who has not read the
  // anti-hallucination spec. An unknown kind falls back to itself rather than
  // to a key nobody wrote.
  const kindKey = `disc.srcKind.${citation.kind}`;
  const kindLabel = citation.kind
    ? (t(kindKey) === kindKey ? citation.kind : t(kindKey))
    : '';
  // "Unknown" is a state too: a chip that claimed every citation was fine
  // would be worse than the raw marker it replaces.
  const state = chipStateFor(verdict);
  const lane = verdict ? laneOf(verdict) : null;
  // Where the file was looked for, in words, before the backend's own reason.
  const where = !verdict
    ? null
    : state === 'lane'
      ? t('disc.srcOrigin.lane', lane ?? '')
      : state === 'verified'
        ? t('disc.srcOrigin.checkout')
        : verdict.status === 'not_found'
          ? t('disc.srcOrigin.notFound')
          : lane !== null
            ? t('disc.srcOrigin.invalidInLane', lane)
            : null;
  const marker = `[src: ${citation.raw}]`;
  const title = verdict
    ? [marker, where, verdict.detail].filter(Boolean).join('\n')
    : marker;

  return (
    <span
      className="disc-src-chip"
      data-kind={citation.kind || undefined}
      data-state={state}
      data-testid="source-citation-chip"
      title={title}
      aria-label={where ? `${kindLabel} ${citation.label} — ${where}`.trim() : undefined}
    >
      {kindLabel && <span className="disc-src-chip-kind">{kindLabel}</span>}
      <span className="disc-src-chip-ref">{citation.label}</span>
      {state === 'lane' && (
        <span className="disc-src-chip-lane" data-testid="source-citation-lane">
          {t('disc.srcOrigin.laneTag', lane ?? '')}
        </span>
      )}
    </span>
  );
}
