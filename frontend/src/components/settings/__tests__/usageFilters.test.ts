import { describe, expect, it } from 'vitest';
import type { UsageReport } from '../../../types/generated';
import {
  ALL_USAGE_FILTER,
  analyzeUsageReport,
  filterUsageReport,
  observedModelCostPerMillion,
  usageAgents,
  usageModels,
} from '../usageFilters';

const report: UsageReport = {
  period_kind: 'daily',
  rows: [{
    period: '2026-08-10',
    agent: 'all',
    models_used: ['claude-sonnet-4-5', 'gpt-5-codex'],
    model_breakdowns: [
      {
        model_name: 'claude-sonnet-4-5', input_tokens: 10,
        output_tokens: 20, cache_creation_tokens: 30,
        cache_read_tokens: 40, total_tokens: 100, cost: 1.25,
      },
      {
        model_name: 'gpt-5-codex', input_tokens: 2,
        output_tokens: 3, cache_creation_tokens: 5,
        cache_read_tokens: 7, total_tokens: 17, cost: 0.5,
      },
    ],
    input_tokens: 12,
    output_tokens: 23,
    cache_creation_tokens: 35,
    cache_read_tokens: 47,
    total_tokens: 117,
    total_cost: 1.75,
  }],
  totals: {
    input_tokens: 12,
    output_tokens: 23,
    cache_creation_tokens: 35,
    cache_read_tokens: 47,
    total_tokens: 117,
    total_cost: 1.75,
  },
  agents_detected: ['claude', 'codex'],
};

describe('usage filters', () => {
  it('calculates the observed cost per million tokens for an exact model', () => {
    expect(observedModelCostPerMillion(report, 'gpt-5-codex')).toBeCloseTo(
      (0.5 / 17) * 1_000_000,
    );
  });

  it('aggregates Claude aliases but does not guess versioned model names', () => {
    const multiModel = structuredClone(report);
    multiModel.rows[0].model_breakdowns.push({
      model_name: 'claude-sonnet-5', input_tokens: 10,
      output_tokens: 10, cache_creation_tokens: 0,
      cache_read_tokens: 0, total_tokens: 20, cost: 0.25,
    });

    expect(observedModelCostPerMillion(multiModel, 'sonnet')).toBeCloseTo(
      (1.5 / 120) * 1_000_000,
    );
    expect(observedModelCostPerMillion(multiModel, 'claude-sonnet-4')).toBeNull();
    expect(observedModelCostPerMillion(multiModel, 'auto')).toBeNull();
  });

  it('reports a zero observed rate for a used local model', () => {
    const withLocal = structuredClone(report);
    withLocal.rows[0].model_breakdowns.push({
      model_name: 'qwen3:8b', input_tokens: 20,
      output_tokens: 10, cache_creation_tokens: 0,
      cache_read_tokens: 0, total_tokens: 30, cost: 0,
    });

    expect(observedModelCostPerMillion(withLocal, 'qwen3:8b')).toBe(0);
  });

  it('derives stable agent and agent-scoped model options', () => {
    expect(usageAgents(report)).toEqual(['claude', 'codex']);
    expect(usageModels(report, 'claude')).toEqual(['claude-sonnet-4-5']);
    expect(usageModels(report, ALL_USAGE_FILTER)).toEqual([
      'claude-sonnet-4-5', 'gpt-5-codex',
    ]);
  });

  it('recalculates every total from the selected model breakdown', () => {
    const filtered = filterUsageReport(report, 'codex', 'gpt-5-codex');

    expect(filtered.rows).toHaveLength(1);
    expect(filtered.rows[0]).toMatchObject({
      input_tokens: 2,
      output_tokens: 3,
      cache_creation_tokens: 5,
      cache_read_tokens: 7,
      total_tokens: 17,
      total_cost: 0.5,
      models_used: ['gpt-5-codex'],
    });
    expect(filtered.totals).toEqual({
      input_tokens: 2,
      output_tokens: 3,
      cache_creation_tokens: 5,
      cache_read_tokens: 7,
      total_tokens: 17,
      total_cost: 0.5,
    });
    expect(filtered.agents_detected).toEqual(['codex']);
  });

  it('returns an empty report for an unavailable model', () => {
    const filtered = filterUsageReport(report, ALL_USAGE_FILTER, 'missing-model');

    expect(filtered.rows).toEqual([]);
    expect(filtered.agents_detected).toEqual([]);
    expect(filtered.totals.total_tokens).toBe(0);
    expect(filtered.totals.total_cost).toBe(0);
  });

  it('identifies usage, total cost and cost-efficiency leaders', () => {
    const analysis = analyzeUsageReport(report);

    expect(analysis?.mostUsed.map(model => model.modelName)).toEqual([
      'claude-sonnet-4-5', 'gpt-5-codex',
    ]);
    expect(analysis?.mostExpensive.map(model => model.modelName)).toEqual([
      'claude-sonnet-4-5', 'gpt-5-codex',
    ]);
    expect(analysis?.leastExpensive.map(model => model.modelName)).toEqual([
      'gpt-5-codex', 'claude-sonnet-4-5',
    ]);
    expect(analysis?.mostUsed[1]).toMatchObject({
      totalTokens: 17,
      totalCost: 0.5,
    });
    expect(analysis?.efficiencyTop.map(model => model.modelName)).toEqual([
      'claude-sonnet-4-5', 'gpt-5-codex',
    ]);
    expect(analysis?.efficiencyTop[0].tokensPerDollar).toBe(80);
  });

  it('ranks a used zero-cost model first and marks its ratio as local', () => {
    const withLocal = structuredClone(report);
    withLocal.rows[0].model_breakdowns.push({
      model_name: 'ollama/qwen3', input_tokens: 20,
      output_tokens: 10, cache_creation_tokens: 0,
      cache_read_tokens: 0, total_tokens: 30, cost: 0,
    });

    const analysis = analyzeUsageReport(withLocal);

    expect(analysis?.efficiencyTop[0].modelName).toBe('ollama/qwen3');
    expect(analysis?.efficiencyTop[0].tokensPerDollar).toBeNull();
  });

  it('aggregates the same model across every period before ranking', () => {
    const multiPeriod = structuredClone(report);
    multiPeriod.rows.push({
      ...structuredClone(report.rows[0]),
      period: '2026-08-11',
      model_breakdowns: [{
        model_name: 'gpt-5-codex', input_tokens: 150,
        output_tokens: 50, cache_creation_tokens: 0,
        cache_read_tokens: 0, total_tokens: 200, cost: 0.1,
      }],
    });

    const analysis = analyzeUsageReport(multiPeriod);

    expect(analysis?.mostUsed[0].modelName).toBe('gpt-5-codex');
    expect(analysis?.mostUsed[0].totalTokens).toBe(217);
    expect(analysis?.mostExpensive[0].modelName).toBe('claude-sonnet-4-5');
  });

  it('keeps one-item rankings when only one model has usage', () => {
    const oneModel = structuredClone(report);
    oneModel.rows[0].model_breakdowns = [oneModel.rows[0].model_breakdowns[0]];

    const analysis = analyzeUsageReport(oneModel);

    expect(analysis?.mostUsed).toHaveLength(1);
    expect(analysis?.mostExpensive).toHaveLength(1);
    expect(analysis?.leastExpensive).toHaveLength(1);
    expect(analysis?.mostUsed[0].modelName).toBe('claude-sonnet-4-5');
  });

  it('does not invent an analysis when no model has usage', () => {
    const empty = structuredClone(report);
    empty.rows = [];

    expect(analyzeUsageReport(empty)).toBeNull();
  });
});
