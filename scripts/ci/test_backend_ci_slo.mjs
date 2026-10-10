import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { SLO_MS, backendChain, comparableSuccessfulHotRuns, copiedJobs, effectiveMeasurementMode, fastLoopDurationMs, formatDuration, runnerMilliseconds, hasRestoredCompiledCache, markdown, percentile, currentBackendChain, summarizeBackendRuns, timingStatus, validateCompiledCacheState } from "./backend_ci_slo.mjs";

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
const firstRun = { run_attempt: 1 };
const summary = summarizeBackendRuns([chainJobs(0, 9), chainJobs(15, 11), chainJobs(32, 12), missChain, chainJobs(50, 9).slice(1)].map((jobs) => ({ run: firstRun, jobs })));
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

assert.equal(currentBackendChain(chainJobs(0, 14)).chain.durationMs, 14 * 60 * 1000);
assert.throws(() => currentBackendChain([]), /exactly one build-backend-tests job, found 0/);
assert.throws(() => currentBackendChain([...chainJobs(0, 9), job("test-backend", 0, 9)]), /exactly one test-backend job, found 2/);
const running = chainJobs(0, 9);
running[3] = { ...running[3], status: "in_progress" };
assert.throws(() => currentBackendChain(running), /test-backend is not complete/);
const noEnd = chainJobs(0, 9);
noEnd[3] = { ...noEnd[3], completed_at: null };
assert.throws(() => currentBackendChain(noEnd), /no valid duration/);

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

// History never recycles a copied chain: a run re-run for another job keeps
// its first attempt's backend chain, which is not a sample of that attempt.
const copiedHistory = { run: { run_attempt: 2, run_started_at: at(80) }, jobs: [...chainJobs(0, 9, { run_attempt: 1 }), job("test-shell", 81, 82, { run_attempt: 2 })] };
assert.equal(summarizeBackendRuns([copiedHistory]).samples.length, 0);
assert.equal(summarizeBackendRuns([{ run: firstRun, jobs: chainJobs(0, 9) }]).samples.length, 1);

// The real entry point, against GitHub API fixtures (fetch replaced at import).
const SCRIPT = fileURLToPath(new URL("./backend_ci_slo.mjs", import.meta.url));
const FETCH_STUB = `data:text/javascript,${encodeURIComponent(`
  const fixtures = JSON.parse(process.env.SLO_FIXTURES);
  globalThis.fetch = async (url) => {
    const { pathname, search } = new URL(url);
    const body = fixtures[pathname.replace(/^\\/repos\\/[^/]+\\/[^/]+/, "") + search];
    return { ok: body !== undefined, status: body === undefined ? 404 : 200, json: async () => body };
  };`)}`;
function observe(run, jobs) {
  const fixtures = {
    "/actions/runs/7/jobs?per_page=100": { jobs },
    "/actions/runs/7": { id: 7, event: "pull_request", head_branch: "feature/ci", ...run },
    "/actions/workflows/ci-test.yml/runs?status=completed&per_page=20": { workflow_runs: [] },
  };
  const env = { ...process.env, SLO_FIXTURES: JSON.stringify(fixtures), GITHUB_RUN_ID: "7", GITHUB_REPOSITORY: "o/r", GITHUB_TOKEN: "t", CI_CACHE_MODE: "hot", CI_COMPILED_CACHE_HIT: "true", CI_COMPILED_CACHE_STATE: "hit" };
  delete env.GITHUB_STEP_SUMMARY;
  return spawnSync(process.execPath, ["--import", FETCH_STUB, SCRIPT], { env, encoding: "utf8" });
}
const attempt2 = { created_at: at(0), run_started_at: at(63), run_attempt: 2 };

// Partial backend re-run: build and partition 2 copied, partition 1 and the
// aggregate re-run green. The observer publishes the chain as unavailable.
const partialBackend = observe(attempt2, [
  job("build-backend-tests", 1, 3, { run_attempt: 1, steps: hit }),
  job("test-backend-partition (1)", 64, 70, { run_attempt: 2 }),
  job("test-backend-partition (2)", 3, 9, { run_attempt: 1 }),
  job("test-backend", 70, 71, { run_attempt: 2 }),
  job("ci-quality-gates", 71, 72, { run_attempt: 2 }),
]);
assert.equal(partialBackend.status, 0, partialBackend.stderr);
assert.match(partialBackend.stdout, /Current backend critical path \| unavailable \(partial re-run: build-backend-tests, test-backend-partition \(2\) copied from an earlier attempt; hot\)/);
assert.match(partialBackend.stdout, /attempt 2 \(from the attempt's start\) \| unavailable \(partial re-run\)/);
assert.match(partialBackend.stdout, /Runner time, jobs run in this attempt \| 8m 0s/);
assert.doesNotMatch(partialBackend.stdout, /\| build-backend-tests \|/);

// Only test-shell re-run: the whole backend chain is the first attempt's.
const shellOnly = observe(attempt2, [
  ...chainJobs(1, 9, { run_attempt: 1 }),
  job("ci-quality-gates", 10, 11, { run_attempt: 1 }),
  job("test-shell", 64, 66, { run_attempt: 2 }),
]);
assert.equal(shellOnly.status, 0, shellOnly.stderr);
assert.match(shellOnly.stdout, /Current backend critical path \| unavailable \(partial re-run: /);
assert.doesNotMatch(shellOnly.stdout, /\| test-backend \| /);
assert.match(shellOnly.stdout, /Runner time, jobs run in this attempt \| 2m 0s/);

// Full re-run 63 minutes later: 9-minute chain, 12-minute loop, no wait counted.
const fullRerun = observe(attempt2, [...chainJobs(64, 9, { run_attempt: 2 }), job("ci-quality-gates", 74, 75, { run_attempt: 2 })]);
assert.equal(fullRerun.status, 0, fullRerun.stderr);
assert.match(fullRerun.stdout, /Current backend critical path \| 9m 0s \(within SLO; hot\)/);
assert.match(fullRerun.stdout, /attempt 2 \(from the attempt's start\) \| 12m 0s/);

// Invalid data stays an error: the current attempt's aggregate has no end.
const broken = chainJobs(64, 9, { run_attempt: 2 });
broken[3] = { ...broken[3], completed_at: null };
const invalid = observe(attempt2, [...broken, job("ci-quality-gates", 74, 75, { run_attempt: 2 })]);
assert.equal(invalid.status, 1);
assert.match(invalid.stderr, /::error title=Backend CI timing unavailable::build-backend-tests to test-backend has no valid duration/);
