import assert from "node:assert/strict";
import { SLO_MS, comparableSuccessfulHotRuns, effectiveMeasurementMode, fastLoopDurationMs, formatDuration, runnerMilliseconds, hasRestoredCompiledCache, markdown, percentile, requireCurrentBackendJob, summarizeBackendJobs, timingStatus, validateCompiledCacheState } from "./backend_ci_slo.mjs";

const at = (minutes) => `2026-08-31T00:${String(minutes).padStart(2, "0")}:00Z`;
const job = (start, end) => ({ name: "test-backend", started_at: at(start), completed_at: at(end) });
assert.equal(formatDuration(SLO_MS), "10m 0s");
assert.equal(percentile([4, 1, 3, 2], 0.5), 2);
assert.equal(percentile([4, 1, 3, 2], 0.95), 4);
const summary = summarizeBackendJobs([job(0, 9), job(15, 26), job(32, 44), { name: "test-frontend", started_at: at(0), completed_at: at(59) }, { name: "test-backend", started_at: "invalid", completed_at: at(1) }]);
assert.equal(summary.samples.length, 3);
assert.equal(summary.medianMs, 11 * 60 * 1000);
assert.equal(summary.p95Ms, 12 * 60 * 1000);
assert.equal(summary.consecutiveBreaches, 2);
assert.deepEqual(summarizeBackendJobs([]), { samples: [], medianMs: null, p95Ms: null, consecutiveBreaches: 0 });
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
assert.equal(hasRestoredCompiledCache({ steps: [{ name: "Record compiled cache hit", conclusion: "success" }] }), true);
assert.equal(hasRestoredCompiledCache({ steps: [{ name: "Record compiled cache warmup miss", conclusion: "success" }] }), false);
assert.equal(hasRestoredCompiledCache({ steps: [{ name: "Record compiled cache hit", conclusion: "failure" }] }), false);

const completedJob = { ...job(0, 14), status: "completed" };
assert.equal(requireCurrentBackendJob([completedJob]).durationMs, 14 * 60 * 1000);
assert.throws(() => requireCurrentBackendJob([]), /found 0/);
assert.throws(() => requireCurrentBackendJob([completedJob, completedJob]), /found 2/);
assert.throws(() => requireCurrentBackendJob([{ ...completedJob, status: "in_progress" }]), /not complete/);
assert.throws(() => requireCurrentBackendJob([{ ...completedJob, completed_at: null }]), /no valid completed duration/);

validateCompiledCacheState("cold", "", false);
validateCompiledCacheState("hot", "miss", false);
validateCompiledCacheState("hot", "hit", true);
assert.throws(() => validateCompiledCacheState("hot", "", false), /invalid or unavailable/);
assert.throws(() => validateCompiledCacheState("hot", "invalid", false), /invalid or unavailable/);
assert.throws(() => validateCompiledCacheState("hot", "hit", false), /outputs disagree/);
assert.throws(() => validateCompiledCacheState("hot", "miss", true), /outputs disagree/);

const runJobs = [
  { name: "test-backend", started_at: at(1), completed_at: at(12) },
  { name: "test-frontend", started_at: at(1), completed_at: at(8) },
  { name: "ci-quality-gates", started_at: at(13), completed_at: at(14) },
  { name: "skipped", started_at: null, completed_at: null },
];
assert.equal(fastLoopDurationMs({ created_at: at(0) }, runJobs), 14 * 60 * 1000);
assert.equal(fastLoopDurationMs({ created_at: at(0) }, runJobs.slice(0, 2)), null);
assert.equal(runnerMilliseconds(runJobs), 19 * 60 * 1000);
const totalsReport = markdown(summary, completedJob, "hot", true, { fastLoopMs: 14 * 60 * 1000, runnerMs: 19 * 60 * 1000 });
assert.match(totalsReport, /Trigger to ci-quality-gates \| 14m 0s/);
assert.match(totalsReport, /Runner time, all jobs of this run \| 19m 0s/);
