import assert from "node:assert/strict";
import { SLO_MS, backendChain, comparableSuccessfulHotRuns, copiedJobs, effectiveMeasurementMode, fastLoopDurationMs, formatDuration, runnerMilliseconds, hasRestoredCompiledCache, markdown, percentile, requireCurrentBackendChain, summarizeBackendRuns, timingStatus, validateCompiledCacheState } from "./backend_ci_slo.mjs";

const at = (minutes) => new Date(Date.UTC(2026, 7, 31, 0, minutes)).toISOString();
const hit = [{ name: "Record compiled cache hit", conclusion: "success" }];
const job = (name, start, end, extra = {}) => ({ name, started_at: at(start), completed_at: at(end), status: "completed", ...extra });
// One run's backend chain: build, two partitions, aggregate, from START for TOTAL minutes.
const chainJobs = (start, total, extra = {}) => [
  job("build-backend-tests", start, start + 2, { steps: hit, ...extra }),
  job("test-backend-partition (1)", start + 2, start + total - 1, extra),
  job("test-backend-partition (2)", start + 2, start + total - 2, extra),
  job("test-backend", start + total - 1, start + total, extra),
];

assert.equal(formatDuration(SLO_MS), "10m 0s");
assert.equal(percentile([4, 1, 3, 2], 0.5), 2);
assert.equal(percentile([4, 1, 3, 2], 0.95), 4);

// The chain spans build start to aggregate end, whatever the partitions do.
assert.equal(backendChain(chainJobs(0, 9)).durationMs, 9 * 60 * 1000);
assert.equal(backendChain(chainJobs(0, 9)).partitions.length, 2);
assert.equal(backendChain(chainJobs(0, 9).slice(1)), null);
const mixedAttempts = chainJobs(0, 9, { run_attempt: 2 });
mixedAttempts[0] = { ...mixedAttempts[0], run_attempt: 1 };
assert.equal(backendChain(mixedAttempts), null, "a chain spanning attempts includes the wait");

const missChain = chainJobs(100, 3);
missChain[0] = { ...missChain[0], steps: [{ name: "Record compiled cache warmup miss", conclusion: "success" }] };
const summary = summarizeBackendRuns([chainJobs(0, 9), chainJobs(15, 11), chainJobs(32, 12), missChain, chainJobs(50, 9).slice(1)]);
assert.equal(summary.samples.length, 3);
assert.equal(summary.medianMs, 11 * 60 * 1000);
assert.equal(summary.p95Ms, 12 * 60 * 1000);
assert.equal(summary.consecutiveBreaches, 2);
assert.deepEqual(summarizeBackendRuns([]), { samples: [], medianMs: null, p95Ms: null, consecutiveBreaches: 0 });
assert.equal(timingStatus(null), "unavailable");
assert.equal(timingStatus(SLO_MS), "within SLO");
assert.equal(timingStatus(SLO_MS + 1), "breach");
assert.equal(effectiveMeasurementMode("hot", true), "hot");
assert.equal(effectiveMeasurementMode("hot", false), "warmup/miss");
assert.equal(effectiveMeasurementMode("cold", false), "cold");
const warmupReport = markdown(summary, null, "warmup/miss", false);
assert.match(warmupReport, /Compiled cache: \*\*miss\*\*/);
assert.match(warmupReport, /unavailable \(unavailable; warmup\/miss\)/);

const currentRun = { id: 5, event: "pull_request", conclusion: null, head_branch: "feature/ci" };
const comparable = comparableSuccessfulHotRuns([
  { id: 1, event: "pull_request", conclusion: "success", head_branch: "feature/ci" },
  { id: 2, event: "workflow_dispatch", conclusion: "success", head_branch: "feature/ci" },
  { id: 3, event: "pull_request", conclusion: "failure", head_branch: "feature/ci" },
  { id: 4, event: "pull_request", conclusion: "success", head_branch: "other-branch" },
  { id: 6, event: "pull_request", conclusion: "cancelled", head_branch: "feature/ci" },
  currentRun,
], currentRun);
assert.deepEqual(comparable.map((run) => run.id), [1]);
assert.equal(hasRestoredCompiledCache({ steps: hit }), true);
assert.equal(hasRestoredCompiledCache({ steps: [{ name: "Record compiled cache warmup miss", conclusion: "success" }] }), false);
assert.equal(hasRestoredCompiledCache({ steps: [{ name: "Record compiled cache hit", conclusion: "failure" }] }), false);

assert.equal(requireCurrentBackendChain(chainJobs(0, 14)).durationMs, 14 * 60 * 1000);
assert.throws(() => requireCurrentBackendChain([]), /exactly one build-backend-tests job, found 0/);
assert.throws(() => requireCurrentBackendChain([...chainJobs(0, 9), job("test-backend", 0, 9)]), /exactly one test-backend job, found 2/);
const running = chainJobs(0, 9);
running[3] = { ...running[3], status: "in_progress" };
assert.throws(() => requireCurrentBackendChain(running), /test-backend is not complete/);
const noEnd = chainJobs(0, 9);
noEnd[3] = { ...noEnd[3], completed_at: null };
assert.throws(() => requireCurrentBackendChain(noEnd), /no valid duration/);

validateCompiledCacheState("cold", "", false);
validateCompiledCacheState("hot", "miss", false);
validateCompiledCacheState("hot", "hit", true);
assert.throws(() => validateCompiledCacheState("hot", "", false), /invalid or unavailable/);
assert.throws(() => validateCompiledCacheState("hot", "invalid", false), /invalid or unavailable/);
assert.throws(() => validateCompiledCacheState("hot", "hit", false), /outputs disagree/);
assert.throws(() => validateCompiledCacheState("hot", "miss", true), /outputs disagree/);

// First attempt: trigger to the gate's verdict.
const runJobs = [
  ...chainJobs(1, 11),
  job("test-frontend", 1, 8),
  job("ci-quality-gates", 13, 14),
  { name: "skipped", started_at: null, completed_at: null },
];
const firstAttempt = { created_at: at(0), run_started_at: at(0), run_attempt: 1 };
assert.equal(fastLoopDurationMs(firstAttempt, runJobs), 14 * 60 * 1000);
assert.equal(fastLoopDurationMs(firstAttempt, runJobs.filter((j) => j.name !== "ci-quality-gates")), null);
assert.equal(runnerMilliseconds(runJobs, firstAttempt), (2 + 8 + 7 + 1 + 7 + 1) * 60 * 1000);

// A full re-run 63 minutes later: measured from the attempt's start, never
// from the first trigger, so the wait between attempts is not counted.
const rerun = { created_at: at(0), run_started_at: at(63), run_attempt: 2 };
const rerunJobs = [...chainJobs(64, 9, { run_attempt: 2 }), job("ci-quality-gates", 74, 75, { run_attempt: 2 })];
assert.equal(fastLoopDurationMs(rerun, rerunJobs), 12 * 60 * 1000);
assert.equal(copiedJobs(rerun, rerunJobs).length, 0);
assert.equal(backendChain(rerunJobs).durationMs, 9 * 60 * 1000);

// "Re-run failed jobs": GitHub copies the green jobs of attempt 1 into attempt 2.
// There is no full loop to measure, and the copies are not this attempt's runner time.
const partial = [
  job("test-frontend", 1, 8, { run_attempt: 1 }),
  ...chainJobs(64, 9, { run_attempt: 2 }),
  job("ci-quality-gates", 74, 75, { run_attempt: 2 }),
];
assert.equal(fastLoopDurationMs(rerun, partial), null);
assert.equal(runnerMilliseconds(partial, rerun), runnerMilliseconds(rerunJobs, rerun));
// Without run_attempt on jobs, a start before the attempt's start marks a copy too.
const partialNoAttempt = partial.map(({ run_attempt, ...rest }) => rest);
assert.equal(copiedJobs(rerun, partialNoAttempt).length, 1);
assert.equal(fastLoopDurationMs(rerun, partialNoAttempt), null);

const totalsReport = markdown(summary, backendChain(runJobs), "hot", true, { fastLoopMs: 14 * 60 * 1000, runnerMs: 26 * 60 * 1000, attempt: 1 });
assert.match(totalsReport, /Trigger to ci-quality-gates, attempt 1 \| 14m 0s/);
assert.match(totalsReport, /Runner time, jobs run in this attempt \| 26m 0s/);
assert.match(totalsReport, /\| test-backend-partition \(2\) \| 7m 0s \|/);
const partialReport = markdown(summary, backendChain(partial), "hot", true, { fastLoopMs: null, runnerMs: 0, attempt: 2 });
assert.match(partialReport, /attempt 2 \(from the attempt's start\) \| unavailable \(partial re-run\)/);
