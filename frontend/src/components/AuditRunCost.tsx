import { useT } from '../lib/I18nContext';
import { formatUsd } from '../lib/audit-cost';
import type { AuditPhaseCost, AuditRunCost as AuditRunCostData } from '../types/generated';

/**
 * KT-1084 — what an audit run consumed, per phase and in total. A phase no
 * agent measured reads "not measured", never 0; a sum over items some of which
 * reported nothing is marked as a floor (`≥`), and an estimated cost as such (`≈`).
 */

function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

const PHASES = ['briefing', 'audit', 'validation', 'total'] as const;

export default function AuditRunCost({ cost }: { cost: AuditRunCostData }) {
  const { t, locale } = useT();
  const notMeasured = t('projects.docAi.auditRecap.cost.notMeasured');
  const part = (value?: number) => (value == null ? '—' : value.toLocaleString(locale));

  const tokensCell = (phase: AuditPhaseCost) => {
    if (phase.items === 0) return { text: '—', title: undefined };
    if (phase.total_tokens == null) return { text: notMeasured, title: undefined };
    const floor = phase.measured_items < phase.items;
    return {
      text: `${floor ? '≥ ' : ''}${fmtTokens(phase.total_tokens)}`,
      title: [
        t('projects.docAi.auditRecap.cost.splitTitle', part(phase.input_tokens), part(phase.output_tokens),
          part(phase.cache_read_tokens), part(phase.cache_write_tokens)),
        floor ? t('projects.docAi.auditRecap.cost.tokensFloorTitle', phase.items - phase.measured_items, phase.items) : '',
        phase.carried_items > 0 ? t('projects.docAi.auditRecap.cost.carriedTitle', phase.carried_items) : '',
      ].filter(Boolean).join('\n'),
    };
  };

  const costCell = (phase: AuditPhaseCost) => {
    if (phase.items === 0) return { text: '—', title: undefined };
    if (phase.cost_usd_micros == null) return { text: notMeasured, title: undefined };
    const usd = formatUsd(phase.cost_usd_micros, locale);
    const floor = phase.cost_unknown_items > 0;
    const text = floor
      ? t('projects.docAi.auditRecap.cost.floor', usd)
      : phase.cost_estimated ? t('auditTimeline.cost.estimated', usd) : t('auditTimeline.cost.short', usd);
    const title = [
      floor ? t('projects.docAi.auditRecap.cost.floorTitle', phase.cost_unknown_items) : '',
      phase.cost_estimated ? t('projects.docAi.auditRecap.cost.estimatedTitle') : '',
    ].filter(Boolean).join('\n');
    return { text, title: title || undefined };
  };

  return (
    <div className="arp-cost" data-testid="audit-recap-cost">
      <h4>{t('projects.docAi.auditRecap.cost.title')}</h4>
      <table className="arp-steps-table">
        <thead>
          <tr>
            <th>{t('projects.docAi.auditRecap.cost.colPhase')}</th>
            <th className="arp-num">{t('projects.docAi.auditRecap.cost.colTokens')}</th>
            <th className="arp-num">{t('projects.docAi.auditRecap.cost.colCost')}</th>
          </tr>
        </thead>
        <tbody>
          {PHASES.map(key => {
            const tokens = tokensCell(cost[key]);
            const usd = costCell(cost[key]);
            return (
              <tr key={key} data-testid={`audit-recap-cost-${key}`} data-total={key === 'total' ? 'true' : 'false'}>
                <td>{key === 'total' ? <strong>{t('projects.docAi.auditRecap.cost.total')}</strong> : t(`projects.docAi.auditRecap.cost.${key}`)}</td>
                <td className="arp-num" title={tokens.title} data-testid={`audit-recap-cost-${key}-tokens`}>{tokens.text}</td>
                <td className="arp-num" title={usd.title} data-testid={`audit-recap-cost-${key}-usd`}>{usd.text}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
