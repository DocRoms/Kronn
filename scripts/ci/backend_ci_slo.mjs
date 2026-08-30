#!/usr/bin/env node
/** Publishes backend CI timing evidence without changing functional gates. */
import assert from "node:assert/strict";

export const BACKEND_JOB = "test-backend";
export const SLO_MS = 15 * 60 * 1000;
export const HISTORY_LIMIT = 20;

function milliseconds(startedAt, completedAt) {
  const start = Date.parse(startedAt ?? "");
  const end = Date.parse(completedAt ?? "");
  return Number.isFinite(start) && Number.isFinite(end) && end >= start ? end - start : null;
}

export function formatDuration(durationMs) {
  if (durationMs === null) return "unavailable";
  const seconds = Math.round(durationMs / 1000);
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}

export function percentile(values, percentileValue) {
  assert(values.length > 0, "percentile requires at least one value");
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil(sorted.length * percentileValue) - 1];
}

export function summarizeBackendJobs(jobs) {
  const samples = jobs.filter((job) => job.name === BACKEND_JOB)
    .map((job) => ({ ...job, durationMs: milliseconds(job.started_at, job.completed_at) }))
    .filter((job) => job.durationMs !== null);
  const durations = samples.map((sample) => sample.durationMs);
  const newestFirst = [...samples].sort((left, right) => Date.parse(right.completed_at) - Date.parse(left.completed_at));
  let consecutiveBreaches = 0;
  for (const sample of newestFirst) {
    if (sample.durationMs <= SLO_MS) break;
    consecutiveBreaches += 1;
  }
  return {
    samples,
    medianMs: durations.length ? percentile(durations, 0.5) : null,
    p95Ms: durations.length ? percentile(durations, 0.95) : null,
    consecutiveBreaches,
  };
}

async function githubJson(path) {
  const repository = process.env.GITHUB_REPOSITORY;
  const token = process.env.GITHUB_TOKEN;
  if (!repository || !token) throw new Error("GITHUB_REPOSITORY and GITHUB_TOKEN are required");
  const response = await fetch(`https://api.github.com/repos/${repository}${path}`, {
    headers: { Accept: "application/vnd.github+json", Authorization: `Bearer ${token}`, "X-GitHub-Api-Version": "2022-11-28" },
  });
  if (!response.ok) throw new Error(`GitHub API ${response.status} for ${path}`);
  return response.json();
}

async function jobsForRun(runId) {
  const payload = await githubJson(`/actions/runs/${runId}/jobs?per_page=100`);
  return payload.jobs ?? [];
}

function markdown(summary, currentJob, mode) {
  const currentDuration = currentJob?.durationMs ?? null;
  const status = currentDuration !== null && currentDuration > SLO_MS ? "breach" : "within SLO";
  const stepRows = (currentJob?.steps ?? []).map((step) => `| ${step.name} | ${formatDuration(milliseconds(step.started_at, step.completed_at))} |`);
  return ["## Backend CI timing", "", `Measurement mode: **${mode}**. The SLO is ${formatDuration(SLO_MS)} for \`${BACKEND_JOB}\`; this report never changes a functional gate.`, "", "| Metric | Value |", "| --- | --- |", `| Current backend critical path | ${formatDuration(currentDuration)} (${status}) |`, `| Sample size | ${summary.samples.length} completed runs |`, `| Median | ${formatDuration(summary.medianMs)} |`, `| P95 | ${formatDuration(summary.p95Ms)} |`, `| Consecutive SLO breaches | ${summary.consecutiveBreaches} |`, "", "### Current backend job steps", "", "| Step | Duration |", "| --- | --- |", ...stepRows, ""].join("\n");
}

async function main() {
  const runId = process.env.GITHUB_RUN_ID;
  const mode = process.env.CI_CACHE_MODE ?? "hot";
  if (!runId) throw new Error("GITHUB_RUN_ID is required");
  const [currentJobs, history] = await Promise.all([jobsForRun(runId), githubJson(`/actions/workflows/ci-test.yml/runs?status=completed&per_page=${HISTORY_LIMIT}`)]);
  const priorRunIds = (history.workflow_runs ?? []).map((run) => String(run.id)).filter((id) => id !== runId).slice(0, HISTORY_LIMIT - 1);
  const priorJobs = await Promise.all(priorRunIds.map(jobsForRun));
  const currentSummary = summarizeBackendJobs(currentJobs);
  const summary = summarizeBackendJobs([...currentJobs, ...priorJobs.flat()]);
  const report = markdown(summary, currentSummary.samples[0], mode);
  process.stdout.write(`${report}\n`);
  if (process.env.GITHUB_STEP_SUMMARY) await (await import("node:fs/promises")).appendFile(process.env.GITHUB_STEP_SUMMARY, `${report}\n`);
  if (currentSummary.samples[0]?.durationMs > SLO_MS) console.log(`::warning title=Backend CI SLO exceeded::${BACKEND_JOB} took ${formatDuration(currentSummary.samples[0].durationMs)} (SLO ${formatDuration(SLO_MS)}); functional gates remain authoritative.`);
}

if (process.argv[1] === new URL(import.meta.url).pathname) main().catch((error) => console.log(`::warning title=Backend CI timing unavailable::${error.message}`));
