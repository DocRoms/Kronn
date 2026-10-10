#!/usr/bin/env node
/** Publishes backend CI timing evidence without changing functional gates. */
import assert from "node:assert/strict";

// The backend chain: one instrumented build, N test partitions, then the
// aggregate that merges coverage and keeps the `test-backend` check name.
export const BACKEND_BUILD_JOB = "build-backend-tests";
export const BACKEND_PARTITION_PREFIX = "test-backend-partition";
export const BACKEND_JOB = "test-backend";
// Build start to aggregate end, on a hot cache.
export const SLO_MS = 10 * 60 * 1000;
export const HISTORY_LIMIT = 20;
export const HOT_CACHE_HIT_STEP = "Record compiled cache hit";
export const GATE_JOB = "ci-quality-gates";

function milliseconds(startedAt, completedAt) {
  const start = Date.parse(startedAt ?? "");
  const end = Date.parse(completedAt ?? "");
  return Number.isFinite(start) && Number.isFinite(end) && end >= start ? end - start : null;
}

function attemptOf(run) {
  return Number(run?.run_attempt ?? 1) || 1;
}

/** Jobs a partial re-run copied from an earlier attempt: they did not run in this one. */
export function copiedJobs(run, jobs) {
  const attempt = attemptOf(run);
  if (attempt < 2) return [];
  const attemptStart = Date.parse(run?.run_started_at ?? "");
  return jobs.filter((job) => (
    (job.run_attempt !== undefined && Number(job.run_attempt) < attempt)
    || (Number.isFinite(attemptStart) && Date.parse(job.started_at ?? "") < attemptStart)
  ));
}

/**
 * Trigger to the aggregate gate's verdict: what a pull request waits. A re-run
 * is measured from its own attempt's start, never from the first trigger, so
 * the wait between attempts is never counted; a partial re-run has no loop.
 */
export function fastLoopDurationMs(run, jobs) {
  const gate = jobs.find((job) => job.name === GATE_JOB);
  if (!gate || copiedJobs(run, jobs).length > 0) return null;
  const start = attemptOf(run) > 1 ? run?.run_started_at : run?.created_at;
  return milliseconds(start, gate.completed_at);
}

/** Billed runner time of this attempt: every job that ran in it, summed. */
export function runnerMilliseconds(jobs, run = {}) {
  const copied = new Set(copiedJobs(run, jobs));
  return jobs.filter((job) => !copied.has(job))
    .reduce((total, job) => total + (milliseconds(job.started_at, job.completed_at) ?? 0), 0);
}

/**
 * Build start to aggregate end in RUN's current attempt, or null when the
 * build or the aggregate is missing, or when any job of the chain was copied
 * from an earlier attempt (the span would include the wait, or not be current).
 */
export function backendChain(jobs, run = {}) {
  const builds = jobs.filter((job) => job.name === BACKEND_BUILD_JOB);
  const aggregates = jobs.filter((job) => job.name === BACKEND_JOB);
  if (builds.length !== 1 || aggregates.length !== 1) return null;
  const partitions = jobs.filter((job) => job.name?.startsWith(BACKEND_PARTITION_PREFIX));
  const chain = [builds[0], ...partitions, aggregates[0]];
  const copied = new Set(copiedJobs(run, jobs));
  if (chain.some((job) => copied.has(job))) return null;
  if (new Set(chain.map((job) => Number(job.run_attempt ?? 1))).size !== 1) return null;
  const durationMs = milliseconds(builds[0].started_at, aggregates[0].completed_at);
  return durationMs === null ? null : { build: builds[0], partitions, aggregate: aggregates[0], durationMs };
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

/** History of hot chains: one { run, jobs } per earlier run, kept when its build restored the cache. */
export function summarizeBackendRuns(runs) {
  const samples = runs.map(({ run, jobs }) => backendChain(jobs, run))
    .filter((chain) => chain !== null && hasRestoredCompiledCache(chain.build))
    .map((chain) => ({ completed_at: chain.aggregate.completed_at, durationMs: chain.durationMs }));
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

/**
 * The current attempt's backend chain, or { chain: null, unavailable } when a
 * re-run copied part or all of it from an earlier attempt: a normal outcome,
 * published as unavailable. Missing, duplicate or unfinished jobs, or a chain
 * of this attempt without a valid duration, are invalid data and throw.
 */
export function currentBackendChain(jobs, run = {}) {
  for (const name of [BACKEND_BUILD_JOB, BACKEND_JOB]) {
    const matches = jobs.filter((job) => job.name === name);
    if (matches.length !== 1) throw new Error(`Expected exactly one ${name} job, found ${matches.length}`);
    if (matches[0].status && matches[0].status !== "completed") {
      throw new Error(`${name} is not complete (status: ${matches[0].status})`);
    }
  }
  const copied = new Set(copiedJobs(run, jobs));
  const chainJobs = jobs.filter((job) => job.name === BACKEND_BUILD_JOB || job.name === BACKEND_JOB
    || job.name?.startsWith(BACKEND_PARTITION_PREFIX));
  const copiedNames = chainJobs.filter((job) => copied.has(job)).map((job) => job.name);
  if (copiedNames.length > 0) {
    return { chain: null, unavailable: `partial re-run: ${copiedNames.join(", ")} copied from an earlier attempt` };
  }
  const chain = backendChain(jobs, run);
  if (chain === null) throw new Error(`${BACKEND_BUILD_JOB} to ${BACKEND_JOB} has no valid duration within one attempt`);
  return { chain, unavailable: null };
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

const durationRow = (item) => `| ${item.name} | ${formatDuration(milliseconds(item.started_at, item.completed_at))} |`;

export function markdown(summary, chain, mode, compiledCacheHit, runTotals = { fastLoopMs: null, runnerMs: null, attempt: 1 }, unavailable = null) {
  const currentDuration = chain?.durationMs ?? null;
  const status = unavailable ?? timingStatus(currentDuration);
  const cacheState = mode === "cold" ? "not applicable" : compiledCacheHit ? "hit" : "miss";
  const attempt = runTotals.attempt ?? 1;
  const loopLabel = `Trigger to ${GATE_JOB}, attempt ${attempt}${attempt > 1 ? " (from the attempt's start)" : ""}`;
  const jobRows = (chain ? [chain.build, ...chain.partitions, chain.aggregate] : []).map(durationRow);
  const stepRows = (chain?.build.steps ?? []).map(durationRow);
  const historyDescription = mode === "cold"
    ? "Current run only; cold measurements are intentionally excluded from historical hot-cache statistics."
    : "Successful pull-request runs from the same head branch whose compiled cache was restored; manual, failed, cancelled, cold, warmup/miss, and other-branch runs are excluded.";
  return [
    "## Backend CI timing", "",
    `Effective measurement mode: **${mode}**. Compiled cache: **${cacheState}**. The SLO is ${formatDuration(SLO_MS)} from \`${BACKEND_BUILD_JOB}\` start to \`${BACKEND_JOB}\` end; this report never changes a functional gate.`,
    `Historical evidence: ${historyDescription}`, "",
    "| Metric | Value |", "| --- | --- |",
    `| Current backend critical path | ${formatDuration(currentDuration)} (${status}; ${mode}) |`,
    `| ${loopLabel} | ${runTotals.fastLoopMs === null && attempt > 1 ? "unavailable (partial re-run)" : formatDuration(runTotals.fastLoopMs)} |`,
    `| Runner time, jobs run in this attempt | ${formatDuration(runTotals.runnerMs)} |`,
    `| Historical hot sample size | ${summary.samples.length} completed runs |`,
    `| Historical hot median | ${formatDuration(summary.medianMs)} |`,
    `| Historical hot P95 | ${formatDuration(summary.p95Ms)} |`,
    `| Historical hot consecutive SLO breaches | ${summary.consecutiveBreaches} |`, "",
    "### Current backend chain", "", "| Job | Duration |", "| --- | --- |", ...jobRows, "",
    "### Build job steps", "", "| Step | Duration |", "| --- | --- |", ...stepRows, "",
  ].join("\n");
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
  const priorRuns = await Promise.all(comparableRuns.map(async (run) => ({ run, jobs: await jobsForRun(String(run.id)) })));
  const { chain, unavailable } = currentBackendChain(currentJobs, currentRun);
  const summary = summarizeBackendRuns(priorRuns);
  const runTotals = {
    fastLoopMs: fastLoopDurationMs(currentRun, currentJobs),
    runnerMs: runnerMilliseconds(currentJobs, currentRun),
    attempt: attemptOf(currentRun),
  };
  const report = markdown(summary, chain, mode, compiledCacheHit, runTotals, unavailable);
  process.stdout.write(`${report}\n`);
  if (process.env.GITHUB_STEP_SUMMARY) await (await import("node:fs/promises")).appendFile(process.env.GITHUB_STEP_SUMMARY, `${report}\n`);
  if (chain && chain.durationMs > SLO_MS) console.log(`::warning title=Backend CI SLO exceeded::the backend chain took ${formatDuration(chain.durationMs)} (SLO ${formatDuration(SLO_MS)}); functional gates remain authoritative.`);
}

if (process.argv[1] === new URL(import.meta.url).pathname) main().catch((error) => {
  console.error(`::error title=Backend CI timing unavailable::${error.message}`);
  process.exitCode = 1;
});
