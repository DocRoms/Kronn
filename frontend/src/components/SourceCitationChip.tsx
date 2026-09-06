// KT-609 — a citation you can read inside a sentence, and still verify.
//
// The exact reference stays on the chip's title, because a citation is
// evidence: if making it pretty makes it less consultable, this failed.
import { useT } from '../lib/I18nContext';
import type { SourceCheck } from '../types/generated';
import type { SourceCitation } from '../lib/sourceCitations';
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
  // Three states, and "unknown" is one of them: a chip that claimed every
  // citation was fine would be worse than the raw marker it replaces.
  const state = verdict?.status === 'verified'
    ? 'verified'
    : verdict && verdict.status !== 'unchecked'
      ? 'suspect'
      : 'unknown';

  return (
    <span
      className="disc-src-chip"
      data-kind={citation.kind || undefined}
      data-state={state}
      data-testid="source-citation-chip"
      title={verdict ? `[src: ${citation.raw}] — ${verdict.detail}` : `[src: ${citation.raw}]`}
    >
      {kindLabel && <span className="disc-src-chip-kind">{kindLabel}</span>}
      <span className="disc-src-chip-ref">{citation.label}</span>
    </span>
  );
}
