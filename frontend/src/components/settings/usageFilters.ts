import type {
  UsageModelBreakdown,
  UsageReport,
  UsageRow,
  UsageTotals,
} from '../../types/generated';

export const ALL_USAGE_FILTER = 'all';
export const CCUSAGE_GITHUB_URL = 'https://github.com/ccusage/ccusage';

export interface UsageModelAnalysis {
  modelName: string;
  agent: string;
  totalTokens: number;
  totalCost: number;
  tokensPerDollar: number | null;
}

export interface UsageAnalysis {
  mostUsed: UsageModelAnalysis[];
  mostExpensive: UsageModelAnalysis[];
  leastExpensive: UsageModelAnalysis[];
  efficiencyTop: UsageModelAnalysis[];
}

const USAGE_MODEL_FAMILY_ALIASES = new Set(['haiku', 'sonnet', 'opus', 'fable']);

export function observedModelCostPerMillion(
  report: UsageReport,
  candidate: string,
): number | null {
  const normalizedCandidate = candidate.trim().toLowerCase();
  if (!normalizedCandidate || normalizedCandidate === 'auto') return null;

  const breakdowns = report.rows.flatMap(row => row.model_breakdowns);
  const exactMatches = breakdowns.filter(
    breakdown => breakdown.model_name.trim().toLowerCase() === normalizedCandidate,
  );
  const matches = exactMatches.length > 0
    ? exactMatches
    : USAGE_MODEL_FAMILY_ALIASES.has(normalizedCandidate)
      ? breakdowns.filter(breakdown => (
          breakdown.model_name
            .trim()
            .toLowerCase()
            .split(/[^a-z0-9]+/)
            .includes(normalizedCandidate)
        ))
      : [];

  const totals = matches.reduce((sum, breakdown) => ({
    tokens: sum.tokens + breakdown.total_tokens,
    cost: sum.cost + breakdown.cost,
  }), { tokens: 0, cost: 0 });

  return totals.tokens > 0
    ? (totals.cost / totals.tokens) * 1_000_000
    : null;
}

export function agentForUsageModel(model: string): string {
  const normalized = model.toLowerCase();
  if (
    normalized.includes('claude')
    || normalized.includes('opus')
    || normalized.includes('sonnet')
    || normalized.includes('haiku')
  ) return 'claude';
  if (
    normalized.includes('gpt')
    || normalized.includes('codex')
    || /^o[134]/.test(normalized)
  ) return 'codex';
  if (normalized.includes('gemini')) return 'gemini';
  return 'other';
}

export function usageAgents(report: UsageReport): string[] {
  return [...new Set(
    report.rows.flatMap(row => row.model_breakdowns.map(
      breakdown => agentForUsageModel(breakdown.model_name),
    )),
  )].sort();
}

export function usageModels(report: UsageReport, agent: string): string[] {
  return [...new Set(
    report.rows.flatMap(row => row.model_breakdowns
      .filter(breakdown => (
        agent === ALL_USAGE_FILTER
        || agentForUsageModel(breakdown.model_name) === agent
      ))
      .map(breakdown => breakdown.model_name)),
  )].sort((left, right) => left.localeCompare(right));
}

function sumBreakdowns(breakdowns: UsageModelBreakdown[]): Omit<UsageTotals, 'total_cost'> & { total_cost: number } {
  return breakdowns.reduce((totals, breakdown) => ({
    input_tokens: totals.input_tokens + (breakdown.input_tokens ?? 0),
    output_tokens: totals.output_tokens + (breakdown.output_tokens ?? 0),
    cache_creation_tokens: totals.cache_creation_tokens + (breakdown.cache_creation_tokens ?? 0),
    cache_read_tokens: totals.cache_read_tokens + (breakdown.cache_read_tokens ?? 0),
    total_tokens: totals.total_tokens + breakdown.total_tokens,
    total_cost: totals.total_cost + breakdown.cost,
  }), {
    input_tokens: 0,
    output_tokens: 0,
    cache_creation_tokens: 0,
    cache_read_tokens: 0,
    total_tokens: 0,
    total_cost: 0,
  });
}

export function filterUsageReport(
  report: UsageReport,
  agent: string,
  model: string,
): UsageReport {
  if (agent === ALL_USAGE_FILTER && model === ALL_USAGE_FILTER) return report;

  const rows = report.rows.flatMap((row): UsageRow[] => {
    const modelBreakdowns = row.model_breakdowns.filter(breakdown => (
      (agent === ALL_USAGE_FILTER || agentForUsageModel(breakdown.model_name) === agent)
      && (model === ALL_USAGE_FILTER || breakdown.model_name === model)
    ));
    if (modelBreakdowns.length === 0) return [];
    const totals = sumBreakdowns(modelBreakdowns);
    return [{
      ...row,
      models_used: modelBreakdowns.map(breakdown => breakdown.model_name),
      model_breakdowns: modelBreakdowns,
      ...totals,
    }];
  });
  const totals = rows.reduce<UsageTotals>((sum, row) => ({
    input_tokens: sum.input_tokens + row.input_tokens,
    output_tokens: sum.output_tokens + row.output_tokens,
    cache_creation_tokens: sum.cache_creation_tokens + row.cache_creation_tokens,
    cache_read_tokens: sum.cache_read_tokens + row.cache_read_tokens,
    total_tokens: sum.total_tokens + row.total_tokens,
    total_cost: sum.total_cost + row.total_cost,
  }), {
    input_tokens: 0,
    output_tokens: 0,
    cache_creation_tokens: 0,
    cache_read_tokens: 0,
    total_tokens: 0,
    total_cost: 0,
  });

  return {
    ...report,
    rows,
    totals,
    agents_detected: usageAgents({ ...report, rows }),
  };
}

export function analyzeUsageReport(report: UsageReport): UsageAnalysis | null {
  const aggregated = new Map<string, UsageModelAnalysis>();
  for (const row of report.rows) {
    for (const breakdown of row.model_breakdowns) {
      const current = aggregated.get(breakdown.model_name) ?? {
        modelName: breakdown.model_name,
        agent: agentForUsageModel(breakdown.model_name),
        totalTokens: 0,
        totalCost: 0,
        tokensPerDollar: null,
      };
      current.totalTokens += breakdown.total_tokens;
      current.totalCost += breakdown.cost;
      aggregated.set(breakdown.model_name, current);
    }
  }

  const models = [...aggregated.values()]
    .filter(model => model.totalTokens > 0)
    .map(model => ({
      ...model,
      tokensPerDollar: model.totalCost > 0
        ? model.totalTokens / model.totalCost
        : null,
    }));
  if (models.length === 0) return null;

  const descending = (
    selector: (model: UsageModelAnalysis) => number,
  ) => [...models].sort((left, right) => (
    selector(right) - selector(left)
    || right.totalTokens - left.totalTokens
    || left.modelName.localeCompare(right.modelName)
  ));
  const leastExpensive = [...models].sort((left, right) => (
    left.totalCost - right.totalCost
    || right.totalTokens - left.totalTokens
    || left.modelName.localeCompare(right.modelName)
  )).slice(0, 2);
  const efficiencyTop = [...models].sort((left, right) => {
    const leftScore = left.tokensPerDollar ?? Number.POSITIVE_INFINITY;
    const rightScore = right.tokensPerDollar ?? Number.POSITIVE_INFINITY;
    return rightScore - leftScore
      || right.totalTokens - left.totalTokens
      || left.modelName.localeCompare(right.modelName);
  }).slice(0, 3);

  return {
    mostUsed: descending(model => model.totalTokens).slice(0, 2),
    mostExpensive: descending(model => model.totalCost).slice(0, 2),
    leastExpensive,
    efficiencyTop,
  };
}
