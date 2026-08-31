#!/usr/bin/env node
/** Publishes backend CI timing evidence without changing functional gates. */
import assert from "node:assert/strict";

export const BACKEND_JOB = "test-backend";
export const SLO_MS = 15 * 60 * 1000;
export const HISTORY_LIMIT = 20;
export const HOT_CACHE_HIT_STEP = "Record compiled cache hit";

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

export function comparableSuccessfulHotRuns(runs, currentRun) {
  return runs.filter((run) => (
    run.id !== currentRun.id
    && run.event === "pull_request"
    && run.conclusion === "success"
    && run.head_branch === currentRun.head_branch
  )).slice(0, HISTORY_LIMIT);
}

export function hasRestoredCompiledCache(job) {
  return job.steps?.some((step) => step.name === HOT_CACHE_HIT_STEP && step.conclusion === "success") ?? false;
}

export function effectiveMeasurementMode(requestedMode, compiledCacheHit) {
  if (requestedMode === "cold") return "cold";
  return compiledCacheHit ? "hot" : "warmup/miss";
}

export function timingStatus(durationMs) {
  if (durationMs === null) return "unavailable";
  return durationMs > SLO_MS ? "breach" : "within SLO";
}

export function requireCurrentBackendJob(jobs) {
  const matches = jobs.filter((job) => job.name === BACKEND_JOB);
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one ${BACKEND_JOB} job, found ${matches.length}`);
  }
  const job = matches[0];
  if (job.status && job.status !== "completed") {
    throw new Error(`${BACKEND_JOB} is not complete (status: ${job.status})`);
  }
  const durationMs = milliseconds(job.started_at, job.completed_at);
  if (durationMs === null) {
    throw new Error(`${BACKEND_JOB} has no valid completed duration`);
  }
  return { ...job, durationMs };
}

export function validateCompiledCacheState(requestedMode, state, compiledCacheHit) {
  if (requestedMode === "cold") return;
  if (state !== "hit" && state !== "miss") {
    throw new Error(`Compiled backend cache state is invalid or unavailable: ${state || "empty"}`);
  }
  if ((state === "hit") !== compiledCacheHit) {
    throw new Error(`Compiled backend cache outputs disagree (state=${state}, hit=${compiledCacheHit})`);
  }
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

export function markdown(summary, currentJob, mode, compiledCacheHit) {
  const currentDuration = currentJob?.durationMs ?? null;
  const status = timingStatus(currentDuration);
  const cacheState = mode === "cold" ? "not applicable" : compiledCacheHit ? "hit" : "miss";
  const stepRows = (currentJob?.steps ?? []).map((step) => `| ${step.name} | ${formatDuration(milliseconds(step.started_at, step.completed_at))} |`);
  const historyDescription = mode === "cold"
    ? "Current run only; cold measurements are intentionally excluded from historical hot-cache statistics."
    : "Successful pull-request runs from the same head branch whose compiled cache was restored; manual, failed, cancelled, cold, warmup/miss, and other-branch runs are excluded.";
  return ["## Backend CI timing", "", `Effective measurement mode: **${mode}**. Compiled cache: **${cacheState}**. The SLO is ${formatDuration(SLO_MS)} for \`${BACKEND_JOB}\`; this report never changes a functional gate.`, `Historical evidence: ${historyDescription}`, "", "| Metric | Value |", "| --- | --- |", `| Current backend critical path | ${formatDuration(currentDuration)} (${status}; ${mode}) |`, `| Historical hot sample size | ${summary.samples.length} completed runs |`, `| Historical hot median | ${formatDuration(summary.medianMs)} |`, `| Historical hot P95 | ${formatDuration(summary.p95Ms)} |`, `| Historical hot consecutive SLO breaches | ${summary.consecutiveBreaches} |`, "", "### Current backend job steps", "", "| Step | Duration |", "| --- | --- |", ...stepRows, ""].join("\n");
}

async function main() {
  const runId = process.env.GITHUB_RUN_ID;
  const requestedMode = process.env.CI_CACHE_MODE ?? "hot";
  const compiledCacheHit = process.env.CI_COMPILED_CACHE_HIT === "true";
  const compiledCacheState = process.env.CI_COMPILED_CACHE_STATE ?? "";
  validateCompiledCacheState(requestedMode, compiledCacheState, compiledCacheHit);
  const mode = effectiveMeasurementMode(requestedMode, compiledCacheHit);
  if (!runId) throw new Error("GITHUB_RUN_ID is required");
  const [currentJobs, currentRun, history] = await Promise.all([
    jobsForRun(runId),
    githubJson(`/actions/runs/${runId}`),
    githubJson(`/actions/workflows/ci-test.yml/runs?status=completed&per_page=${HISTORY_LIMIT}`),
  ]);
  const comparableRuns = mode === "hot"
    ? comparableSuccessfulHotRuns(history.workflow_runs ?? [], currentRun)
    : [];
  const priorRunIds = comparableRuns.map((run) => String(run.id));
  const priorJobs = await Promise.all(priorRunIds.map(jobsForRun));
  const currentJob = requireCurrentBackendJob(currentJobs);
  const summary = summarizeBackendJobs(priorJobs.flat().filter(hasRestoredCompiledCache));
  const report = markdown(summary, currentJob, mode, compiledCacheHit);
  process.stdout.write(`${report}\n`);
  if (process.env.GITHUB_STEP_SUMMARY) await (await import("node:fs/promises")).appendFile(process.env.GITHUB_STEP_SUMMARY, `${report}\n`);
  if (currentJob.durationMs > SLO_MS) console.log(`::warning title=Backend CI SLO exceeded::${BACKEND_JOB} took ${formatDuration(currentJob.durationMs)} (SLO ${formatDuration(SLO_MS)}); functional gates remain authoritative.`);
}

if (process.argv[1] === new URL(import.meta.url).pathname) main().catch((error) => {
  console.error(`::error title=Backend CI timing unavailable::${error.message}`);
  process.exitCode = 1;
});
